//! Project settings, a dialog in four pages: general (the name), the members
//! (added, removed, and each one's CLI, model, effort and prompt), who may
//! hand work to whom, and the team's rules (`[settings]`). Saved with
//! `lila::settings` into the repo's `.lila/`; lilad reads it again on save.

use crate::app::LilaApp;
use crate::data::{self, Project};
use crate::dialog::{self, Act, Nav, Shell, block, group, row, stepper, value_text};
use crate::ui::s;
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::checkbox::Checkbox;
use gpui::component::input::{Input, InputState, Textarea, TextareaState};
use gpui::component::select::{Select, SelectGroup, SelectState, SearchableVec};
use gpui::component::IndexPath;
use gpui::component::menu::{DropdownMenu, PopupMenuItem};
use gpui::component::scroll::ScrollableElement;
use gpui::component::switch::Switch;
use gpui::component::tag::Tag;
use gpui::component::{ActiveTheme, Disableable, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui::prelude::*;
use gpui::*;
use lila::settings::{MemberSettings, ProjectSettings};
use lila_core::{Runtime, Settings};
use serde_json::json;

/// Models offered per CLI (one already in a config stays offered too).
pub const CLAUDE_MODELS: &[&str] = &["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-fable-5-1"];
pub const CODEX_MODELS: &[&str] = &["gpt-6.1-sol", "gpt-6-astra", "gpt-6.1-terra", "gpt-6-luna"];
/// `--effort` (claude) and `model_reasoning_effort` (codex) both take these.
pub const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Members,
    Assign,
    Rules,
}

impl Page {
    /// For `LILA_APP_SETTINGS=<page>` (screenshots).
    #[cfg(feature = "shots")]
    pub fn parse(s: &str) -> Page {
        match s {
            "members" => Page::Members,
            "assign" => Page::Assign,
            "rules" => Page::Rules,
            _ => Page::General,
        }
    }
}

/// The numbers in `[settings]`, with the bounds the form keeps them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rule {
    StallMinutes,
    StartupSecs,
    MaxRounds,
    MaxDepth,
}

impl Rule {
    fn get(self, s: &Settings) -> i64 {
        match self {
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
            Rule::StallMinutes => s.stall_minutes = v as u64,
            Rule::StartupSecs => s.startup_timeout_secs = v as u64,
            Rule::MaxRounds => s.max_rounds = v as u32,
            Rule::MaxDepth => s.max_chain_depth = v as u32,
        }
    }

    fn range(self) -> (i64, i64) {
        match self {
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
            Rule::StallMinutes => ("无回应提醒", "分钟", "成员手上有任务却停下这么久没汇报，就提醒它一次；还不回应就告诉 PM。"),
            Rule::MaxRounds => ("来回返工上限", "轮", "两个成员在一条任务链上来回退回超过这个轮数，就交给 PM 决定。"),
            Rule::MaxDepth => ("子任务层数上限", "层", "任务链 T1 → T2 → T3 … 最多几层，防止无限拆分。"),
            Rule::StartupSecs => ("启动等待", "秒", "成员启动后这么久还没报告状态，就在收件箱提醒你（多半卡在信任目录或登录）。"),
        }
    }
}

pub struct MemberForm {
    pub base: MemberSettings,
    /// The model, and so the CLI.
    pub model: ModelSelect,
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
    /// The member open on the members page.
    pub selected: usize,
    pub error: Option<String>,
    pub saving: bool,
}

fn member_form(m: &MemberSettings, new: bool, window: &mut Window, cx: &mut Context<LilaApp>) -> MemberForm {
    MemberForm {
        base: m.clone(),
        model: model_select(&m.model, m.runtime, window, cx),
        effort: m.effort.clone(),
        prompt: cx.new(|cx| TextareaState::new(window, cx).rows(12).default_value(m.prompt.clone())),
        id: new.then(|| cx.new(|cx| InputState::new(window, cx).placeholder("成员编号").default_value(m.id.clone()))),
    }
}

/// Effort as a segmented control, "默认" first.
pub fn effort_control(id: String, effort: Option<&str>, pick: impl Fn(&mut LilaApp, Option<String>) + 'static, cx: &mut Context<LilaApp>) -> AnyElement {
    let mut options = vec!["默认"];
    options.extend(EFFORTS);
    let selected = effort.and_then(|e| EFFORTS.iter().position(|x| *x == e)).map(|p| p + 1).unwrap_or(0);
    dialog::segmented(&id, &options, selected, move |this, i, _, _| pick(this, if i == 0 { None } else { Some(EFFORTS[i - 1].to_owned()) }), cx)
}

pub type ModelSelect = Entity<SelectState<SearchableVec<SelectGroup<SharedString>>>>;

/// The model as one choice from two groups, Claude Code and ChatGPT (Codex):
/// the group is the CLI. A model not offered is kept, in its CLI's group.
pub fn model_select(model: &str, runtime: Runtime, window: &mut Window, cx: &mut Context<LilaApp>) -> ModelSelect {
    let mut claude: Vec<SharedString> = CLAUDE_MODELS.iter().map(|m| SharedString::from(*m)).collect();
    let mut codex: Vec<SharedString> = CODEX_MODELS.iter().map(|m| SharedString::from(*m)).collect();
    let model = model.trim();
    if !model.is_empty() && !CLAUDE_MODELS.contains(&model) && !CODEX_MODELS.contains(&model) {
        match runtime {
            Runtime::Claude => claude.push(model.to_owned().into()),
            Runtime::Codex => codex.push(model.to_owned().into()),
        }
    }
    let at = |list: &[SharedString], section: usize| list.iter().position(|m| m.as_ref() == model).map(|row| IndexPath::new(row).section(section));
    let selected = at(&claude, 0).or_else(|| at(&codex, 1));
    let groups = SearchableVec::new(vec![SelectGroup::new("Claude Code").items(claude), SelectGroup::new("ChatGPT · Codex").items(codex)]);
    cx.new(|cx| SelectState::new(groups, selected, window, cx))
}

/// The CLI and model a [`model_select`] has chosen.
pub fn selected_model(select: &ModelSelect, cx: &App) -> Option<(Runtime, String)> {
    let s = select.read(cx);
    let ix = s.selected_index(cx)?;
    let model = s.selected_value()?.to_string();
    Some((if ix.section == 1 { Runtime::Codex } else { Runtime::Claude }, model))
}

pub fn model_control(select: &ModelSelect) -> AnyElement {
    div().w(px(250.)).child(Select::new(select).small().placeholder("选择模型")).into_any_element()
}

impl MemberForm {
    /// The CLI and model chosen now.
    fn choice(&self, cx: &App) -> (Runtime, String) {
        selected_model(&self.model, cx).unwrap_or((self.base.runtime, self.base.model.clone()))
    }

    fn changed(&self, cx: &App) -> bool {
        let (runtime, model) = self.choice(cx);
        let own_prompt = self.prompt.read(cx).value().trim() != self.base.role_prompt.trim();
        runtime != self.base.role_runtime || model != self.base.role_model || self.effort != self.base.role_effort || own_prompt
    }
}

impl LilaApp {
    pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings_at(Page::General, window, cx);
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
        self.prefs = None;
        self.dialog_offset = Point::default();
        self.close_task(cx);
        let title = cx.new(|cx| InputState::new(window, cx).placeholder(base.name.clone()).default_value(base.title.clone()));
        let members = base.members.iter().map(|m| member_form(m, false, window, cx)).collect();
        self.settings = Some(Form { project, value: base.clone(), base, title, members, page, selected: 0, error: None, saving: false });
        cx.notify();
    }

    fn settings_member(&mut self, ix: usize) -> Option<&mut MemberForm> {
        self.settings.as_mut().and_then(|f| f.members.get_mut(ix))
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
                    f.selected = f.members.len() - 1;
                    f.page = Page::Members;
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
                f.selected = f.selected.min(f.members.len().saturating_sub(1));
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
            (m.runtime, m.model) = form.choice(cx);
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

    fn general_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(f) = self.settings.as_ref() else { return div().into_any_element() };
        let (title, name, cwd) = (f.title.clone(), f.base.name.clone(), data::tilde(&f.project.cwd));
        dialog::scroll(
            "settings-general",
            vec![
                group(None, vec![row("项目名称", Some("窗口里显示的名字，可以用中文。".into()), div().w(px(250.)).child(Input::new(&title).small()), cx)], cx),
                group(
                    Some("位置"),
                    vec![
                        row("项目编号", Some("命令行和终端名用的是它，创建后不能改。".into()), value_text(name, cx), cx),
                        row("目录", None, value_text(cwd, cx), cx),
                        row("配置文件", Some("这里的设置都写在这个文件里，可以随代码一起提交。".into()), value_text(".lila/project.toml", cx), cx),
                    ],
                    cx,
                ),
            ],
        )
    }

    fn members_page(&mut self, running: &[String], cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(f) = self.settings.as_ref() else { return div().into_any_element() };
        let owner = f.value.owner.clone();
        let selected = f.selected.min(f.members.len().saturating_sub(1));
        // One PM per project: its role is not offered again.
        let pm_role = f.value.owner_role().map(str::to_owned);
        let roles: Vec<(String, String)> = f.value.roles.iter().filter(|r| Some(&r.id) != pm_role.as_ref()).map(|r| (r.id.clone(), r.file.name.clone())).collect();
        struct Item {
            id: String,
            count: u32,
            role: String,
            new: bool,
            changed: bool,
            owner: bool,
            running: bool,
        }
        let items: Vec<Item> = f
            .members
            .iter()
            .zip(&f.value.members)
            .map(|(m, v)| {
                Item {
                    id: v.id.clone(),
                    count: v.count,
                    role: m.base.role_name.clone(),
                    new: m.id.is_some(),
                    changed: m.changed(cx) || v.count != m.base.count,
                    owner: v.ids().contains(&owner),
                    running: v.ids().iter().any(|i| running.contains(i)),
                }
            })
            .collect();

        let weak = cx.entity().downgrade();
        let list = v_flex()
            .w(px(200.))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(theme.border)
            .child(
                v_flex().id("member-list").flex_1().min_h_0().overflow_y_scrollbar().p_2().gap_0p5().children(items.iter().enumerate().map(|(ix, it)| {
                    let active = ix == selected;
                    h_flex()
                        .id(SharedString::from(format!("member-item-{ix}")))
                        .px_2()
                        .py_1p5()
                        .gap_2()
                        .rounded_md()
                        .cursor_pointer()
                        .when(active, |d| d.bg(theme.accent).text_color(theme.accent_foreground))
                        .when(!active, |d| d.hover(|d| d.bg(theme.accent.opacity(0.5))))
                        .child(Icon::new(IconName::Bot).small().text_color(if it.running { theme.green } else { theme.muted_foreground }))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(h_flex().gap_1().child(div().text_sm().font_medium().truncate().child(if it.id.is_empty() { "（未命名）".to_owned() } else { it.id.clone() })).when(it.count > 1, |d| d.child(div().text_xs().text_color(theme.muted_foreground).child(format!("×{}", it.count)))))
                                .child(div().text_xs().text_color(theme.muted_foreground).truncate().child(it.role.clone())),
                        )
                        .when(it.owner, |d| d.child(Tag::secondary().xsmall().child("PM")))
                        .when(it.new, |d| d.child(Tag::success().xsmall().child("新")))
                        .when(it.changed && !it.new, |d| d.child(dot(theme.blue)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(f) = this.settings.as_mut() {
                                f.selected = ix;
                            }
                            cx.notify();
                        }))
                })),
            )
            .child(
                div().p_2().border_t_1().border_color(theme.border).child(Button::new("add-member").ghost().small().w_full().icon(IconName::Plus).label("添加成员").dropdown_menu(move |mut menu, _, _| {
                    menu = menu.label("按角色添加");
                    for (id, name) in &roles {
                        let (weak, id) = (weak.clone(), id.clone());
                        menu = menu.item(PopupMenuItem::new(format!("{name}  ·  {id}")).on_click(move |_, window, cx| {
                            let _ = weak.update(cx, |this, cx| this.add_member(&id, window, cx));
                        }));
                    }
                    menu
                })),
            );

        let editor = match items.get(selected) {
            Some(it) => self.member_editor(selected, it.owner, it.running, cx),
            None => div().p_6().text_color(theme.muted_foreground).child("还没有成员。").into_any_element(),
        };
        h_flex().size_full().child(list).child(v_flex().flex_1().min_w_0().h_full().child(editor)).into_any_element()
    }

    fn member_editor(&mut self, ix: usize, is_owner: bool, is_running: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(m) = self.settings.as_ref().and_then(|f| f.members.get(ix)) else { return div().into_any_element() };
        let (base, effort, select, prompt, id_input) = (m.base.clone(), m.effort.clone(), m.model.clone(), m.prompt.clone(), m.id.clone());
        let (runtime, model) = m.choice(cx);
        let own_prompt = prompt.read(cx).value().trim() != base.role_prompt.trim();
        let changed = m.changed(cx);
        let (shown_id, count) = self.settings.as_ref().and_then(|f| f.value.members.get(ix)).map(|v| (v.id.clone(), v.count)).unwrap_or_default();

        let header = h_flex()
            .gap_3()
            .child(div().size(px(36.)).flex_none().rounded_lg().bg(theme.accent).flex().items_center().justify_center().child(Icon::new(IconName::Bot).text_color(theme.accent_foreground)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(h_flex().gap_2().child(div().text_base().font_semibold().child(shown_id.clone())).when(count > 1, |d| d.child(div().text_xs().text_color(theme.muted_foreground).child(format!("×{count}")))).when(is_owner, |d| d.child(Tag::secondary().xsmall().child("PM"))))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(format!("角色：{}（{}）{}", base.role_name, base.role, if is_running { " · 正在运行" } else { "" }))),
            )
            .when(!is_owner, |d| {
                d.child(Button::new(SharedString::from(format!("remove-{ix}"))).outline().small().label("移出团队").tooltip(if is_running { "保存后停掉它的终端，没做完的任务会取消" } else { "保存后从项目里去掉" }).on_click(cx.listener(move |this, _, _, cx| this.remove_member(ix, cx))))
            });

        let mut rows = Vec::new();
        if let Some(input) = &id_input {
            rows.push(row("成员编号", Some("a-z 0-9 - _，字母开头".into()), div().w(px(250.)).child(Input::new(input).small()), cx));
        }
        let cli = match runtime {
            Runtime::Claude => "Claude Code",
            Runtime::Codex => "Codex（ChatGPT）",
        };
        let model_about = if model != base.role_model { format!("{cli} · 角色默认 {}", base.role_model) } else { cli.to_owned() };
        rows.push(row("模型", Some(model_about), model_control(&select), cx));
        rows.push(row(
            "努力程度",
            (effort != base.role_effort).then(|| format!("角色默认 {}", base.role_effort.as_deref().unwrap_or("CLI 默认"))),
            effort_control(
                format!("effort-{ix}"),
                effort.as_deref(),
                move |this, e| {
                    if let Some(m) = this.settings_member(ix) {
                        m.effort = e;
                    }
                },
                cx,
            ),
            cx,
        ));
        let prompt_row = block(
            "预设提示词",
            Some(if own_prompt { format!("只给这个成员用，保存在 .lila/prompts/{}.md", base.id) } else { format!("和角色「{}」的默认提示词一样", base.role_name) }),
            Textarea::new(&prompt),
            own_prompt.then(|| Button::new(SharedString::from(format!("prompt-reset-{ix}"))).ghost().xsmall().icon(IconName::Undo2).label("恢复角色默认").on_click(cx.listener(move |this, _, window, cx| this.reset_prompt(ix, window, cx))).into_any_element()),
            cx,
        );

        let count_row = if is_owner {
            row("同时工作", Some("每个项目只有一个 PM。".into()), value_text("1 个", cx), cx)
        } else {
            let names = lila_core::expand_ids(&shown_id, count.max(1) + 1);
            let down: Option<Act> = (count > 1).then(|| Box::new(move |this: &mut LilaApp, cx: &mut Context<LilaApp>| this.settings_edit(cx, |v| v.set_count(ix, count - 1))) as Act);
            let up: Option<Act> = (count < 9).then(|| Box::new(move |this: &mut LilaApp, cx: &mut Context<LilaApp>| this.settings_edit(cx, |v| v.set_count(ix, count + 1))) as Act);
            row(
                "同时工作",
                Some(format!("同时开几个。多一个就多一个终端（{}），更快，也更费额度。", names.last().cloned().unwrap_or_default())),
                stepper(&format!("count-{ix}"), format!("{} 个", count.max(1)), down, up, cx),
                cx,
            )
        };
        let mut body = vec![header.into_any_element(), group(Some("运行"), rows, cx), group(Some("分工"), vec![count_row], cx), group(Some("提示词"), vec![prompt_row], cx)];
        if changed && is_running {
            body.push(notice(IconName::Info, theme.yellow, "它正在运行：保存后，侧栏和终端标题会出现「重启」，重启后才用上新设置。", cx));
        }
        if id_input.is_some() {
            body.push(notice(IconName::Info, theme.muted_foreground, "新成员保存后不会自动启动：点侧栏的「启动团队」。", cx));
        }
        dialog::scroll(SharedString::from(format!("member-editor-{ix}")), body)
    }

    fn assign_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(f) = self.settings.as_ref() else { return div().into_any_element() };
        let v = &f.value;
        let ids = v.member_ids();
        let owner = v.owner.clone();
        let rows: Vec<(String, Vec<String>, bool)> = ids.iter().map(|m| (m.clone(), v.targets(m), v.assign.contains_key(m))).collect();
        const NAME: f32 = 130.;
        const CELL: f32 = 84.;
        let head = h_flex()
            .h(px(36.))
            .border_b_1()
            .border_color(theme.border)
            .child(div().w(px(NAME)).flex_none().px_3().text_xs().text_color(theme.muted_foreground).child("派活的人 ↓"))
            .children(ids.iter().map(|to| div().w(px(CELL)).flex_none().text_xs().font_medium().text_center().truncate().child(to.clone())))
            .child(div().flex_1());
        let body: Vec<AnyElement> = rows
            .into_iter()
            .enumerate()
            .map(|(n, (from, targets, own))| {
                let is_owner = from == owner;
                let reset = from.clone();
                h_flex()
                    .h(px(40.))
                    .when(n % 2 == 1, |d| d.bg(theme.muted.opacity(0.35)))
                    .child(h_flex().w(px(NAME)).flex_none().px_3().gap_1().child(div().text_sm().font_medium().truncate().child(from.clone())).when(is_owner, |d| d.child(Tag::secondary().xsmall().child("PM"))))
                    .children(ids.iter().map(|to| {
                        let cell = div().w(px(CELL)).flex_none().flex().justify_center();
                        if *to == from {
                            return cell.child(div().text_xs().text_color(theme.muted_foreground.opacity(0.5)).child("—")).into_any_element();
                        }
                        let (a, b) = (from.clone(), to.clone());
                        cell.child(Checkbox::new(SharedString::from(format!("assign-{from}-{to}"))).checked(is_owner || targets.contains(to)).disabled(is_owner).on_click(cx.listener(move |this, on: &bool, _, cx| {
                            let on = *on;
                            this.settings_edit(cx, |v| v.set_target(&a, &b, on))
                        })))
                        .into_any_element()
                    }))
                    .child(div().flex_1())
                    .child(div().w(px(110.)).flex_none().flex().justify_end().pr_2().when(own, |d| {
                        d.child(Button::new(SharedString::from(format!("assign-reset-{from}"))).ghost().xsmall().icon(IconName::Undo2).label("恢复默认").on_click(cx.listener(move |this, _, _, cx| this.settings_edit(cx, |v| v.reset_targets(&reset)))))
                    }))
                    .into_any_element()
            })
            .collect();
        let table = v_flex().rounded_lg().border_1().border_color(theme.border).bg(dialog::card(cx)).overflow_hidden().child(head).children(body);
        dialog::scroll(
            "settings-assign",
            vec![
                table.into_any_element(),
                v_flex()
                    .gap_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("勾选表示这一行的成员可以把任务派给那一列的成员。")
                    .child("没改过的行跟着角色的默认规则走（加了新成员会自动带上）；改过的行可以恢复默认。你和 PM 可以派给任何人。")
                    .into_any_element(),
            ],
        )
    }

    fn rule_row(&mut self, rule: Rule, now: &Settings, was: &Settings, cx: &mut Context<Self>) -> AnyElement {
        let (title, unit, about) = rule.text();
        let v = rule.get(now);
        let (lo, hi) = rule.range();
        let step = rule.step();
        let down: Option<Act> = (v > lo).then(|| Box::new(move |this: &mut LilaApp, cx: &mut Context<LilaApp>| this.settings_edit(cx, |s| rule.set(&mut s.settings, v - step))) as Act);
        let up: Option<Act> = (v < hi).then(|| Box::new(move |this: &mut LilaApp, cx: &mut Context<LilaApp>| this.settings_edit(cx, |s| rule.set(&mut s.settings, v + step))) as Act);
        let title = if v != rule.get(was) { format!("{title}  ·  已改") } else { title.to_owned() };
        let control = stepper(&format!("{rule:?}"), format!("{v} {unit}"), down, up, cx);
        row(title, Some(about.to_owned()), control, cx)
    }

    fn rules_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(f) = self.settings.as_ref() else { return div().into_any_element() };
        let (now, was) = (f.value.settings.clone(), f.base.settings.clone());
        let theme = cx.theme().clone();
        let stall = self.rule_row(Rule::StallMinutes, &now, &was, cx);
        let rounds = self.rule_row(Rule::MaxRounds, &now, &was, cx);
        let depth = self.rule_row(Rule::MaxDepth, &now, &was, cx);
        let startup = self.rule_row(Rule::StartupSecs, &now, &was, cx);
        let fresh = now.fresh_session;
        let fresh_row = row(
            if fresh != was.fresh_session { "换任务时开新会话  ·  已改" } else { "换任务时开新会话" },
            Some("成员要做另一条任务链的活时，先重启它的 CLI，上下文从零开始；同一条链上的返工沿用原会话。PM 不受影响。".into()),
            Switch::new("fresh-session").checked(fresh).on_click(cx.listener(move |this, on: &bool, _, cx| {
                let on = *on;
                this.settings_edit(cx, |s| s.settings.fresh_session = on)
            })),
            cx,
        );
        dialog::scroll(
            "settings-rules",
            vec![
                group(Some("派发"), vec![fresh_row], cx),
                group(Some("防失控"), vec![stall, rounds, depth], cx),
                group(Some("启动"), vec![startup], cx),
                div().text_xs().text_color(theme.muted_foreground).child("只保存和默认值不同的部分，写在 .lila/project.toml 的 [settings] 里。").into_any_element(),
            ],
        )
    }

    pub fn settings_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.settings.as_ref()?;
        self.sync_ids(cx);
        let theme = cx.theme().clone();
        let running: Vec<String> = self.members().iter().filter(|m| m["status"] != "gone").map(|m| s(m, "id")).collect();
        let f = self.settings.as_ref()?;
        let (error, saving, label, page, id) = (f.error.clone(), f.saving, f.project.label(), f.page, f.project.id.clone());
        let kept = f.value.member_ids();
        let stopping: Vec<String> = f.base.member_ids().into_iter().filter(|m| !kept.contains(m) && running.contains(m)).collect();
        let (count, overrides) = (f.value.member_ids().len(), f.value.assign.len());
        let dirty = self.settings_value(cx).is_some_and(|(_, v)| self.settings.as_ref().is_some_and(|f| v != f.base));

        let (title, description, body) = match page {
            Page::General => ("通用", "项目的名字和位置。", self.general_page(cx)),
            Page::Members => ("成员", "团队里有谁，各自用什么 CLI、模型和提示词。没改过的部分跟着角色走。", self.members_page(&running, cx)),
            Page::Assign => ("指派规则", "成员之间可以互相派任务。这里决定谁能派给谁。", self.assign_page(cx)),
            Page::Rules => ("运行规则", "lilad 派发任务、提醒和防止失控时用的规则。同时工作几个，在「成员」里按成员设置。", self.rules_page(cx)),
        };
        let note = if let Some(e) = error {
            Some(h_flex().gap_2().text_color(theme.red).child(Icon::new(IconName::CircleAlert).small()).child(div().truncate().child(e)).into_any_element())
        } else if !stopping.is_empty() {
            Some(h_flex().gap_2().text_color(theme.yellow).child(Icon::new(IconName::TriangleAlert).small()).child(div().truncate().child(format!("保存后会停掉 {}，没做完的任务会取消。", stopping.join("、")))).into_any_element())
        } else if dirty {
            Some(div().text_color(theme.muted_foreground).child("有未保存的更改").into_any_element())
        } else {
            None
        };
        let go = |p: Page| -> Act {
            Box::new(move |this: &mut LilaApp, _| {
                if let Some(f) = this.settings.as_mut() {
                    f.page = p;
                }
            })
        };
        let nav = vec![
            (Nav { id: "nav-general", icon: IconName::Settings2, label: "通用", active: page == Page::General, badge: None }, go(Page::General)),
            (Nav { id: "nav-members", icon: IconName::Bot, label: "成员", active: page == Page::Members, badge: Some(count.to_string()) }, go(Page::Members)),
            (Nav { id: "nav-assign", icon: IconName::Network, label: "指派规则", active: page == Page::Assign, badge: (overrides > 0).then(|| format!("改 {overrides}")) }, go(Page::Assign)),
            (Nav { id: "nav-rules", icon: IconName::LayoutDashboard, label: "运行规则", active: page == Page::Rules, badge: None }, go(Page::Rules)),
        ];
        let heading = v_flex()
            .gap_1()
            .child(div().text_xs().text_color(theme.muted_foreground).child("项目设置"))
            .child(h_flex().gap_2().child(crate::ui::avatar(&id, cx)).child(div().font_semibold().truncate().child(label)))
            .into_any_element();
        Some(self.dialog(
            Shell {
                id: "project-settings",
                heading,
                nav,
                title: title.into(),
                description: description.into(),
                body,
                note,
                saving,
                on_save: Box::new(|this, cx| this.save_settings(cx)),
                on_close: Box::new(|this, _| this.settings = None),
            },
            cx,
        ))
    }
}

fn dot(color: Hsla) -> Div {
    div().size(px(6.)).flex_none().rounded_full().bg(color)
}

pub fn notice(icon: IconName, color: Hsla, text: &str, cx: &App) -> AnyElement {
    let t = cx.theme();
    h_flex().gap_2().px_3().py_2().rounded_md().bg(t.muted.opacity(0.5)).text_xs().child(Icon::new(icon).small().text_color(color)).child(div().flex_1().child(text.to_owned())).into_any_element()
}
