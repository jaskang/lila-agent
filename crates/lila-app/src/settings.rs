//! Project settings in the window, in three pages: the members (added,
//! removed, and each one's CLI, model, effort and prompt), who may hand work
//! to whom, and the team's rules (`[settings]`). Saved with `lila::settings`
//! into the repo's `.lila/`; lilad reads it again on save.

use crate::app::LilaApp;
use crate::data::{self, Project};
use crate::ui::s;
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::{Input, InputState, Textarea, TextareaState};
use gpui::component::scroll::ScrollableElement;
use gpui::component::switch::Switch;
use gpui::component::tag::Tag;
use gpui::component::{ActiveTheme, Disableable, IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui::prelude::*;
use gpui::*;
use lila::settings::{MemberSettings, ProjectSettings};
use lila_core::{Runtime, Settings};
use serde_json::json;

/// Models offered per CLI; any other can be typed in.
const CLAUDE_MODELS: &[&str] = &["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-fable-5-1"];
const CODEX_MODELS: &[&str] = &["gpt-6.1-sol", "gpt-6-astra", "gpt-6.1-terra", "gpt-6-luna"];
/// `--effort` (claude) and `model_reasoning_effort` (codex) both take these.
const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

fn models(runtime: Runtime) -> &'static [&'static str] {
    match runtime {
        Runtime::Claude => CLAUDE_MODELS,
        Runtime::Codex => CODEX_MODELS,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Members,
    Assign,
    Rules,
}

impl Page {
    /// For `LILA_APP_SETTINGS=<page>` (screenshots).
    #[cfg(feature = "shots")]
    pub fn parse(s: &str) -> Page {
        match s {
            "assign" => Page::Assign,
            "rules" => Page::Rules,
            _ => Page::Members,
        }
    }
}

/// The numbers in `[settings]`, with the bounds the form keeps them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rule {
    MaxWorking,
    StallMinutes,
    StartupSecs,
    MaxRounds,
    MaxDepth,
}

impl Rule {
    const ALL: [Rule; 5] = [Rule::MaxWorking, Rule::StallMinutes, Rule::MaxRounds, Rule::MaxDepth, Rule::StartupSecs];

    fn get(self, s: &Settings) -> i64 {
        match self {
            Rule::MaxWorking => s.max_working.into(),
            Rule::StallMinutes => s.stall_minutes as i64,
            Rule::StartupSecs => s.startup_timeout_secs as i64,
            Rule::MaxRounds => s.max_rounds.into(),
            Rule::MaxDepth => s.max_chain_depth.into(),
        }
    }

    fn set(self, s: &mut Settings, v: i64) {
        let (lo, hi) = self.range();
        let v = v.clamp(lo, hi);
        match self {
            Rule::MaxWorking => s.max_working = v as u32,
            Rule::StallMinutes => s.stall_minutes = v as u64,
            Rule::StartupSecs => s.startup_timeout_secs = v as u64,
            Rule::MaxRounds => s.max_rounds = v as u32,
            Rule::MaxDepth => s.max_chain_depth = v as u32,
        }
    }

    fn range(self) -> (i64, i64) {
        match self {
            Rule::MaxWorking => (1, 16),
            Rule::StallMinutes => (1, 120),
            Rule::StartupSecs => (5, 600),
            Rule::MaxRounds => (1, 20),
            Rule::MaxDepth => (1, 32),
        }
    }

    fn step(self) -> i64 {
        if self == Rule::StartupSecs { 5 } else { 1 }
    }

    /// Title, unit, explanation.
    fn text(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Rule::MaxWorking => ("同时工作的成员", "人", "PM 以外，同时在做任务的成员上限。订阅额度够用可以调高，任务会更快，额度也用得更快。"),
            Rule::StallMinutes => ("无回应提醒", "分钟", "成员手上有任务、却停下来这么久没有汇报，就提醒它一次；还不回应就告诉 PM。"),
            Rule::MaxRounds => ("来回返工上限", "轮", "两个成员在一条任务链上来回退回（比如开发 → 测试 → 开发）超过这个轮数，就交给 PM 决定。"),
            Rule::MaxDepth => ("子任务层数上限", "层", "任务链 T1 → T2 → T3 … 最多能有几层，防止无限拆分。"),
            Rule::StartupSecs => ("启动等待", "秒", "成员启动后这么久还没报告状态，就在收件箱提醒你（多半卡在信任目录或登录）。"),
        }
    }
}

pub struct MemberForm {
    pub base: MemberSettings,
    pub runtime: Runtime,
    pub model: Entity<InputState>,
    pub effort: Option<String>,
    pub prompt: Entity<TextareaState>,
    /// Added in this form: its id can still be typed.
    pub id: Option<Entity<InputState>>,
}

pub struct Form {
    pub project: Project,
    /// As loaded, to tell what this form changed.
    pub base: ProjectSettings,
    /// Members, rules and `[settings]` as the form has them now (the inputs hold the rest).
    pub value: ProjectSettings,
    pub title: Entity<InputState>,
    pub members: Vec<MemberForm>,
    pub page: Page,
    pub error: Option<String>,
    pub saving: bool,
}

/// One member's form as it is drawn (read out so the listeners can borrow the app).
struct MemberSnapshot {
    base: MemberSettings,
    runtime: Runtime,
    model: String,
    effort: Option<String>,
    model_input: Entity<InputState>,
    prompt: Entity<TextareaState>,
    id_input: Option<Entity<InputState>>,
    is_owner: bool,
}

fn field(title: &str, cx: &App) -> Div {
    v_flex().gap_1().child(div().text_xs().font_semibold().text_color(cx.theme().muted_foreground).child(title.to_owned()))
}

fn member_form(m: &MemberSettings, new: bool, window: &mut Window, cx: &mut Context<LilaApp>) -> MemberForm {
    MemberForm {
        base: m.clone(),
        runtime: m.runtime,
        model: cx.new(|cx| InputState::new(window, cx).placeholder("模型名").default_value(m.model.clone())),
        effort: m.effort.clone(),
        prompt: cx.new(|cx| TextareaState::new(window, cx).rows(10).default_value(m.prompt.clone())),
        id: new.then(|| cx.new(|cx| InputState::new(window, cx).placeholder("成员编号").default_value(m.id.clone()))),
    }
}

impl LilaApp {
    pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings_at(Page::Members, window, cx);
    }

    pub fn open_settings_at(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project() else { return };
        let base = match lila::settings::load(std::path::Path::new(&project.cwd), &self.paths().roles()) {
            Ok(b) => b,
            Err(e) => {
                self.notice = Some(format!("读取项目设置失败：{e:#}"));
                cx.notify();
                return;
            }
        };
        self.draft = None;
        self.close_task(cx);
        let title = cx.new(|cx| InputState::new(window, cx).placeholder(base.name.clone()).default_value(base.title.clone()));
        let members = base.members.iter().map(|m| member_form(m, false, window, cx)).collect();
        self.settings = Some(Form { project, value: base.clone(), base, title, members, page, error: None, saving: false });
        cx.notify();
    }

    fn settings_member(&mut self, ix: usize) -> Option<&mut MemberForm> {
        self.settings.as_mut().and_then(|f| f.members.get_mut(ix))
    }

    fn pick_runtime(&mut self, ix: usize, runtime: Runtime, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.settings_member(ix) else { return };
        if m.runtime == runtime {
            return;
        }
        m.runtime = runtime;
        // The model must be one that CLI runs: the role's if it uses this CLI, else the first offered.
        let model = if m.base.role_runtime == runtime { m.base.role_model.clone() } else { models(runtime)[0].to_owned() };
        let input = m.model.clone();
        input.update(cx, |s, cx| s.set_value(model, window, cx));
        cx.notify();
    }

    fn pick_model(&mut self, ix: usize, model: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.settings_member(ix) else { return };
        let input = m.model.clone();
        input.update(cx, |s, cx| s.set_value(model.to_owned(), window, cx));
        cx.notify();
    }

    fn reset_prompt(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.settings_member(ix) else { return };
        let (input, text) = (m.prompt.clone(), m.base.role_prompt.clone());
        input.update(cx, |s, cx| s.set_value(text, window, cx));
        cx.notify();
    }

    pub fn add_member(&mut self, role: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.settings.as_mut() else { return };
        match f.value.add_member(role) {
            Ok(_) => {
                let m = f.value.members.last().expect("just added").clone();
                let form = member_form(&m, true, window, cx);
                if let Some(f) = self.settings.as_mut() {
                    f.members.push(form);
                }
            }
            Err(e) => f.error = Some(format!("{e:#}")),
        }
        cx.notify();
    }

    fn remove_member(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(f) = self.settings.as_mut() else { return };
        let Some(id) = f.value.members.get(ix).map(|m| m.id.clone()) else { return };
        match f.value.remove_member(&id) {
            Ok(()) => {
                f.members.remove(ix);
            }
            Err(e) => f.error = Some(format!("{e:#}")),
        }
        cx.notify();
    }

    /// New members' ids follow their inputs, so the rules page names them right.
    fn sync_ids(&mut self, cx: &App) {
        let Some(f) = self.settings.as_mut() else { return };
        for ix in 0..f.members.len() {
            let Some(input) = &f.members[ix].id else { continue };
            let typed = input.read(cx).value().trim().to_owned();
            if f.value.members.get(ix).is_some_and(|m| m.id != typed) {
                f.value.rename_member(ix, &typed);
            }
        }
    }

    /// What the form says now, as settings to save.
    fn settings_value(&self, cx: &App) -> Option<(Project, ProjectSettings)> {
        let f = self.settings.as_ref()?;
        let mut out = f.value.clone();
        out.title = f.title.read(cx).value().trim().to_owned();
        for (m, form) in out.members.iter_mut().zip(&f.members) {
            m.runtime = form.runtime;
            m.model = form.model.read(cx).value().trim().to_owned();
            m.effort = form.effort.clone();
            m.prompt = form.prompt.read(cx).value().to_string();
        }
        Some((f.project.clone(), out))
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        self.sync_ids(cx);
        let Some((project, value)) = self.settings_value(cx) else { return };
        if let Some(f) = &mut self.settings {
            f.saving = true;
            f.error = None;
        }
        cx.notify();
        let paths = self.paths().clone();
        let daemon = self.snap.daemon;
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move {
                    lila::settings::save(std::path::Path::new(&project.cwd), &paths.roles(), &value)?;
                    // lilad reads it again: members on old settings are marked, removed ones stopped.
                    // Saved either way: a lilad that can't reload picks it up when it restarts.
                    let warn = if daemon { data::act(&paths, &project, "project.reload", json!({}), false).err() } else { None };
                    anyhow::Ok(warn.map(|e| format!("设置已保存，但 lilad 没能重新读取（{e:#}）。重启 lilad 后生效：lila daemon stop，再启动团队。")))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(warn) => {
                        this.settings = None;
                        this.notice = warn;
                    }
                    Err(e) => {
                        if let Some(f) = &mut this.settings {
                            f.saving = false;
                            f.error = Some(format!("{e:#}"));
                        }
                    }
                }
                this.refresh_now();
                cx.notify();
            });
        })
        .detach();
    }

    fn settings_edit(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut ProjectSettings)) {
        if let Some(f) = self.settings.as_mut() {
            edit(&mut f.value);
        }
        cx.notify();
    }

    fn members_page(&mut self, running: &[String], cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let Some(f) = self.settings.as_ref() else { return Vec::new() };
        let owner = f.value.owner.clone();
        let roles: Vec<(String, String)> = f.value.roles.iter().map(|r| (r.id.clone(), r.file.name.clone())).collect();
        let rows: Vec<(usize, MemberSnapshot)> = f
            .members
            .iter()
            .enumerate()
            .map(|(ix, m)| {
                (
                    ix,
                    MemberSnapshot {
                        base: m.base.clone(),
                        runtime: m.runtime,
                        model: m.model.read(cx).value().to_string(),
                        effort: m.effort.clone(),
                        model_input: m.model.clone(),
                        prompt: m.prompt.clone(),
                        id_input: m.id.clone(),
                        is_owner: m.base.ids().contains(&owner),
                    },
                )
            })
            .collect();

        let mut out: Vec<AnyElement> = rows
            .into_iter()
            .map(|(ix, MemberSnapshot { base, runtime, model, effort, model_input, prompt, id_input, is_owner })| {
                let own_prompt = prompt.read(cx).value().trim() != base.role_prompt.trim();
                let changed = runtime != base.role_runtime || model.trim() != base.role_model || effort != base.role_effort || own_prompt;
                let is_running = running.iter().any(|r| *r == base.id || r.starts_with(&format!("{}-", base.id)));
                let is_new = id_input.is_some();
                let chip = |id: String, text: String, on: bool| Button::new(SharedString::from(id)).xsmall().when(on, |b| b.primary()).when(!on, |b| b.outline()).label(text);
                v_flex()
                    .p_3()
                    .gap_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(if is_new { theme.primary.opacity(0.6) } else { theme.border })
                    .bg(theme.background)
                    .child(
                        h_flex()
                            .gap_2()
                            .map(|d| match &id_input {
                                Some(input) => d.child(div().w(px(180.)).child(Input::new(input).small())),
                                None => d.child(div().font_semibold().child(base.id.clone())),
                            })
                            .when(base.count > 1, |d| d.child(div().text_xs().text_color(theme.muted_foreground).child(format!("×{}", base.count))))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(format!("角色：{}", base.role_name)))
                            .when(is_owner, |d| d.child(Tag::secondary().xsmall().child("PM")))
                            .child(div().flex_1())
                            .when(is_new, |d| d.child(Tag::success().xsmall().child("新成员")))
                            .when(changed && !is_new, |d| d.child(Tag::info().xsmall().child("已改")))
                            .when(!is_owner, |d| {
                                d.child(
                                    Button::new(SharedString::from(format!("remove-{ix}")))
                                        .xsmall()
                                        .ghost()
                                        .label("移出")
                                        .tooltip(if is_running { "移出团队：保存后停掉它的终端，没做完的任务会取消" } else { "移出团队" })
                                        .on_click(cx.listener(move |this, _, _, cx| this.remove_member(ix, cx))),
                                )
                            }),
                    )
                    .child(
                        field("运行方式", cx).child(
                            h_flex()
                                .gap_1()
                                .child(chip(format!("rt-{ix}-claude"), "Claude Code".into(), runtime == Runtime::Claude).on_click(cx.listener(move |this, _, window, cx| this.pick_runtime(ix, Runtime::Claude, window, cx))))
                                .child(chip(format!("rt-{ix}-codex"), "Codex".into(), runtime == Runtime::Codex).on_click(cx.listener(move |this, _, window, cx| this.pick_runtime(ix, Runtime::Codex, window, cx)))),
                        ),
                    )
                    .child(
                        field("模型", cx)
                            .child(h_flex().gap_1().flex_wrap().children(models(runtime).iter().map(|m| {
                                let pick = *m;
                                chip(format!("model-{ix}-{m}"), m.to_string(), model.trim() == *m).on_click(cx.listener(move |this, _, window, cx| this.pick_model(ix, pick, window, cx)))
                            })))
                            .child(Input::new(&model_input).small()),
                    )
                    .child(
                        field("努力程度", cx).child(
                            h_flex()
                                .gap_1()
                                .flex_wrap()
                                .child(chip(format!("effort-{ix}-default"), "CLI 默认".into(), effort.is_none()).on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(m) = this.settings_member(ix) {
                                        m.effort = None;
                                    }
                                    cx.notify();
                                })))
                                .children(EFFORTS.iter().map(|e| {
                                    let pick = e.to_string();
                                    chip(format!("effort-{ix}-{e}"), e.to_string(), effort.as_deref() == Some(*e)).on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(m) = this.settings_member(ix) {
                                            m.effort = Some(pick.clone());
                                        }
                                        cx.notify();
                                    }))
                                })),
                        ),
                    )
                    .child(
                        field("预设提示词", cx)
                            .child(Textarea::new(&prompt))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(if own_prompt { format!("只给这个成员用，保存在 .lila/prompts/{}.md", base.id) } else { format!("和角色「{}」的默认提示词一样", base.role_name) })
                                    .child(div().flex_1())
                                    .when(own_prompt, |d| d.child(Button::new(SharedString::from(format!("prompt-reset-{ix}"))).xsmall().ghost().label("恢复角色默认").on_click(cx.listener(move |this, _, window, cx| this.reset_prompt(ix, window, cx))))),
                            ),
                    )
                    .when(changed && is_running, |d| d.child(div().text_xs().text_color(theme.yellow).child("它正在运行：保存后要重启它才会用上新设置。")))
                    .when(is_new, |d| d.child(div().text_xs().text_color(theme.muted_foreground).child("保存后不会自动启动：点侧栏的「启动团队」。编号用小写字母、数字、- 和 _，以字母开头。")))
                    .into_any_element()
            })
            .collect();

        out.push(
            v_flex()
                .gap_2()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_dashed()
                .border_color(theme.border)
                .child(div().text_xs().font_semibold().text_color(theme.muted_foreground).child("添加成员"))
                .child(h_flex().gap_1().flex_wrap().children(roles.into_iter().map(|(id, name)| {
                    let pick = id.clone();
                    Button::new(SharedString::from(format!("add-{id}"))).xsmall().outline().icon(IconName::Plus).label(format!("{name}（{id}）")).on_click(cx.listener(move |this, _, window, cx| this.add_member(&pick, window, cx)))
                })))
                .child(div().text_xs().text_color(theme.muted_foreground).child("新成员先照搬角色的设置，添加后可以在上面的卡片里改。"))
                .into_any_element(),
        );
        out
    }

    fn assign_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let Some(f) = self.settings.as_ref() else { return Vec::new() };
        let v = &f.value;
        let ids = v.member_ids();
        let rows: Vec<(String, Vec<String>, bool)> = ids.iter().map(|m| (m.clone(), v.targets(m), v.assign.contains_key(m))).collect();
        let owner = v.owner.clone();
        let mut out = vec![div().text_xs().text_color(theme.muted_foreground).child("每一行是一个成员可以把任务派给谁。没改过的行跟着角色的默认规则走；你和 PM 可以派给任何人。").into_any_element()];
        for (from, targets, own) in rows {
            let is_owner = from == owner;
            let chips: Vec<AnyElement> = ids
                .iter()
                .filter(|t| **t != from)
                .map(|to| {
                    let on = targets.contains(to);
                    let (a, b) = (from.clone(), to.clone());
                    Button::new(SharedString::from(format!("assign-{from}-{to}")))
                        .xsmall()
                        .when(on, |x| x.primary())
                        .when(!on, |x| x.outline())
                        .label(to.clone())
                        .disabled(is_owner)
                        .on_click(cx.listener(move |this, _, _, cx| this.settings_edit(cx, |v| v.set_target(&a, &b, !on))))
                        .into_any_element()
                })
                .collect();
            let reset = from.clone();
            out.push(
                v_flex()
                    .p_3()
                    .gap_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().font_semibold().child(from.clone()))
                            .child(div().text_xs().text_color(theme.muted_foreground).child("可以派给"))
                            .when(is_owner, |d| d.child(Tag::secondary().xsmall().child("PM · 所有人")))
                            .child(div().flex_1())
                            .when(own, |d| {
                                d.child(Tag::info().xsmall().child("已改")).child(
                                    Button::new(SharedString::from(format!("assign-reset-{from}")))
                                        .xsmall()
                                        .ghost()
                                        .icon(IconName::Undo2)
                                        .tooltip("恢复角色默认")
                                        .on_click(cx.listener(move |this, _, _, cx| this.settings_edit(cx, |v| v.reset_targets(&reset)))),
                                )
                            }),
                    )
                    .child(h_flex().gap_1().flex_wrap().children(chips))
                    .when(targets.is_empty() && !is_owner, |d| d.child(div().text_xs().text_color(theme.muted_foreground).child("不能派任务：做完只向派活的人汇报。")))
                    .into_any_element(),
            );
        }
        out
    }

    fn rules_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let Some(f) = self.settings.as_ref() else { return Vec::new() };
        let (now, was) = (f.value.settings.clone(), f.base.settings.clone());
        let card = || v_flex().p_3().gap_1().rounded_lg().border_1().border_color(theme.border).bg(theme.background);
        let mut out: Vec<AnyElement> = Rule::ALL
            .into_iter()
            .map(|rule| {
                let (title, unit, about) = rule.text();
                let v = rule.get(&now);
                let (lo, hi) = rule.range();
                let step = rule.step();
                let key = format!("{rule:?}");
                card()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().font_semibold().child(title))
                            .when(v != rule.get(&was), |d| d.child(Tag::info().xsmall().child("已改")))
                            .child(div().flex_1())
                            .child(Button::new(SharedString::from(format!("{key}-down"))).xsmall().outline().icon(IconName::Minus).disabled(v <= lo).on_click(cx.listener(move |this, _, _, cx| this.settings_edit(cx, |s| rule.set(&mut s.settings, v - step)))))
                            .child(div().w(px(64.)).text_center().child(format!("{v} {unit}")))
                            .child(Button::new(SharedString::from(format!("{key}-up"))).xsmall().outline().icon(IconName::Plus).disabled(v >= hi).on_click(cx.listener(move |this, _, _, cx| this.settings_edit(cx, |s| rule.set(&mut s.settings, v + step))))),
                    )
                    .child(div().text_xs().text_color(theme.muted_foreground).child(about))
                    .into_any_element()
            })
            .collect();
        let fresh = now.fresh_session;
        out.insert(
            1,
            card()
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().font_semibold().child("换任务时开新会话"))
                        .when(fresh != was.fresh_session, |d| d.child(Tag::info().xsmall().child("已改")))
                        .child(div().flex_1())
                        .child(Switch::new("fresh-session").checked(fresh).on_click(cx.listener(move |this, on: &bool, _, cx| {
                            let on = *on;
                            this.settings_edit(cx, |s| s.settings.fresh_session = on)
                        }))),
                )
                .child(div().text_xs().text_color(theme.muted_foreground).child("成员要做另一条任务链的活时，先重启它的 CLI，上下文从零开始；同一条链上的返工沿用原会话。PM 不受影响。"))
                .into_any_element(),
        );
        out
    }

    pub fn settings_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.sync_ids(cx);
        let theme = cx.theme().clone();
        let f = self.settings.as_ref()?;
        let (title, error, saving, name, label, page) = (f.title.clone(), f.error.clone(), f.saving, f.base.name.clone(), f.project.label(), f.page);
        let running: Vec<String> = self.members().iter().filter(|m| m["status"] != "gone").map(|m| s(m, "id")).collect();
        let kept = f.value.member_ids();
        let stopping: Vec<String> = f.base.member_ids().into_iter().filter(|m| !kept.contains(m) && running.contains(m)).collect();

        let body = match page {
            Page::Members => {
                let mut v = vec![
                    field("项目名称", cx)
                        .child(Input::new(&title))
                        .child(div().text_xs().text_color(theme.muted_foreground).child(format!("窗口里显示的名字，可以用中文。项目编号 {name} 不变，命令行和终端名用的是它。")))
                        .into_any_element(),
                ];
                v.extend(self.members_page(&running, cx));
                v
            }
            Page::Assign => self.assign_page(cx),
            Page::Rules => self.rules_page(cx),
        };
        let tab = |id: &'static str, text: &'static str, p: Page, cx: &mut Context<Self>| {
            Button::new(id).small().when(page == p, |b| b.primary()).when(page != p, |b| b.ghost()).label(text).on_click(cx.listener(move |this, _, _, cx| {
                if let Some(f) = this.settings.as_mut() {
                    f.page = p;
                }
                cx.notify();
            }))
        };

        Some(
            v_flex()
                .id("settings")
                .size_full()
                .text_color(theme.popover_foreground)
                .child(
                    h_flex()
                        .h(px(40.))
                        .px_3()
                        .flex_none()
                        .gap_2()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(div().font_semibold().child("项目设置"))
                        .child(div().text_sm().text_color(theme.muted_foreground).truncate().child(label))
                        .child(div().flex_1())
                        .child(Button::new("settings-close").small().ghost().label("关闭").on_click(cx.listener(|this, _, _, cx| {
                            this.settings = None;
                            cx.notify();
                        }))),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .px_3()
                        .py_2()
                        .gap_1()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(tab("page-members", "成员", Page::Members, cx))
                        .child(tab("page-assign", "指派规则", Page::Assign, cx))
                        .child(tab("page-rules", "运行规则", Page::Rules, cx)),
                )
                .child(
                    v_flex()
                        .id(SharedString::from(format!("settings-{page:?}")))
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scrollbar()
                        .p_4()
                        .gap_4()
                        .children(body)
                        .child(div().text_xs().text_color(theme.muted_foreground).child("只保存和默认值不同的部分，写在仓库的 .lila/project.toml 里，可以随代码一起提交。")),
                )
                .child(
                    v_flex()
                        .flex_none()
                        .p_3()
                        .gap_2()
                        .border_t_1()
                        .border_color(theme.border)
                        .when(!stopping.is_empty(), |d| d.child(div().text_xs().text_color(theme.yellow).child(format!("保存后会停掉 {} 的终端，它们没做完的任务会取消，并通知派活的人。", stopping.join("、")))))
                        .when_some(error, |d, e| d.child(div().p_2().rounded_md().border_l_2().border_color(theme.red).bg(theme.red.opacity(0.12)).text_sm().child(e)))
                        .child(
                            h_flex()
                                .gap_2()
                                .child(Button::new("settings-save").primary().label(if saving { "保存中…" } else { "保存" }).disabled(saving).on_click(cx.listener(|this, _, _, cx| this.save_settings(cx))))
                                .child(Button::new("settings-cancel").ghost().label("取消").on_click(cx.listener(|this, _, _, cx| {
                                    this.settings = None;
                                    cx.notify();
                                }))),
                        ),
                )
                .into_any_element(),
        )
    }
}
