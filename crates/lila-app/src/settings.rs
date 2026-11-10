//! Project settings, a dialog in two pages: general (the name and the team's
//! rules, `[settings]`) and the members (added, removed, and each one's CLI,
//! model, effort and prompt). Who may hand work to whom stays in
//! `.lila/project.toml` (`[assign]`). Saved with `lila::settings` into the
//! repo's `.lila/`; lilad reads it again on save.

use crate::app::LilaApp;
use crate::data::{self, Project};
use crate::dialog::{self, Act, Nav, Shell, block, group, row, stepper, value_text};
use crate::ui::s;
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::{Input, InputState, Textarea, TextareaState};
use gpui::component::select::{Select, SelectGroup, SelectState, SearchableVec};
use gpui::component::IndexPath;
use gpui::component::menu::{DropdownMenu, PopupMenuItem};
use gpui::component::scroll::ScrollableElement;
use gpui::component::switch::Switch;
use gpui::component::{ActiveTheme, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use crate::style::Control as _;
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
}

impl Page {
    /// For `LILA_APP_SETTINGS=<page>` (screenshots).
    #[cfg(feature = "shots")]
    pub fn parse(s: &str) -> Page {
        match s {
            "members" => Page::Members,
            _ => Page::General,
        }
    }
}

/// The numbers in `[settings]`, with the bounds the form keeps them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The rest of `[settings]` (startup timeout, chain depth…) is edited in the file.
enum Rule {
    Parallel,
    StallMinutes,
    MaxRounds,
}

impl Rule {
    fn get(self, s: &Settings) -> i64 {
        match self {
            Rule::Parallel => s.parallel.into(),
            Rule::StallMinutes => s.stall_minutes as i64,
            Rule::MaxRounds => s.max_rounds.into(),
        }
    }

    fn set(self, s: &mut Settings, v: i64) {
        let (lo, hi) = self.range();
        let v = v.clamp(lo, hi);
        match self {
            Rule::Parallel => s.parallel = v as u32,
            Rule::StallMinutes => s.stall_minutes = v as u64,
            Rule::MaxRounds => s.max_rounds = v as u32,
        }
    }

    fn range(self) -> (i64, i64) {
        match self {
            // 0 (chains share the folder) is for the file: a project that is not a git repository shares it anyway.
            Rule::Parallel => (1, 6),
            Rule::StallMinutes => (1, 120),
            Rule::MaxRounds => (1, 20),
        }
    }

    /// Title, unit.
    fn text(self) -> (&'static str, &'static str) {
        match self {
            Rule::Parallel => ("同时进行", "件"),
            Rule::StallMinutes => ("无回应提醒", "分钟"),
            Rule::MaxRounds => ("返工上限", "轮"),
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
    /// `[settings] check`: what Lila runs after the work, before the tester.
    pub check: Entity<InputState>,
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

/// The model as one choice from groups, Claude Code and ChatGPT (Codex), and omp
/// when the role runs on it (its models are omp's to list): the group is the
/// runtime. A model not offered is kept, in its runtime's group.
pub fn model_select(model: &str, runtime: Runtime, window: &mut Window, cx: &mut Context<LilaApp>) -> ModelSelect {
    let mut claude: Vec<SharedString> = CLAUDE_MODELS.iter().map(|m| SharedString::from(*m)).collect();
    let mut codex: Vec<SharedString> = CODEX_MODELS.iter().map(|m| SharedString::from(*m)).collect();
    let mut omp: Vec<SharedString> = Vec::new();
    let model = model.trim();
    if runtime == Runtime::Omp && !model.is_empty() {
        omp.push(model.to_owned().into());
    } else if !model.is_empty() && !CLAUDE_MODELS.contains(&model) && !CODEX_MODELS.contains(&model) {
        match runtime {
            Runtime::Claude => claude.push(model.to_owned().into()),
            Runtime::Codex | Runtime::Omp => codex.push(model.to_owned().into()),
        }
    }
    let at = |list: &[SharedString], section: usize| list.iter().position(|m| m.as_ref() == model).map(|row| IndexPath::new(row).section(section));
    let selected = if runtime == Runtime::Omp { at(&omp, 2) } else { at(&claude, 0).or_else(|| at(&codex, 1)) };
    let mut groups = vec![SelectGroup::new("Claude Code").items(claude), SelectGroup::new("ChatGPT · Codex").items(codex)];
    if !omp.is_empty() {
        groups.push(SelectGroup::new("omp").items(omp));
    }
    let groups = SearchableVec::new(groups);
    cx.new(|cx| SelectState::new(groups, selected, window, cx))
}

/// The CLI and model a [`model_select`] has chosen.
pub fn selected_model(select: &ModelSelect, cx: &App) -> Option<(Runtime, String)> {
    let s = select.read(cx);
    let ix = s.selected_index(cx)?;
    let model = s.selected_value()?.to_string();
    Some((
        match ix.section {
            1 => Runtime::Codex,
            2 => Runtime::Omp,
            _ => Runtime::Claude,
        },
        model,
    ))
}

pub fn model_control(select: &ModelSelect) -> AnyElement {
    div().w(px(280.)).child(Select::new(select).select_control().placeholder("选择模型")).into_any_element()
}

impl MemberForm {
    /// The CLI and model chosen now.
    fn choice(&self, cx: &App) -> (Runtime, String) {
        selected_model(&self.model, cx).unwrap_or((self.base.runtime, self.base.model.clone()))
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
        let check = cx.new(|cx| InputState::new(window, cx).default_value(base.settings.check.clone()));
        let members = base.members.iter().map(|m| member_form(m, false, window, cx)).collect();
        self.settings = Some(Form { project, value: base.clone(), base, title, check, members, page, selected: 0, error: None, saving: false });
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
        out.settings.check = f.check.read(cx).value().trim().to_owned();
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
        let (title, check, cwd) = (f.title.clone(), f.check.clone(), data::tilde(&f.project.cwd));
        let now = f.value.settings.clone();
        let rules: Vec<AnyElement> = [Rule::Parallel, Rule::StallMinutes, Rule::MaxRounds].into_iter().map(|r| self.rule_row(r, &now, cx)).collect();
        let fresh = now.fresh_session;
        let fresh_row = row(
            "换任务时开新会话",
            None,
            Switch::new("fresh-session").checked(fresh).on_click(cx.listener(move |this, on: &bool, _, cx| {
                let on = *on;
                this.settings_edit(cx, |s| s.settings.fresh_session = on)
            })),
            cx,
        );
        let mut team = vec![row("检查命令", None, div().w(px(280.)).child(Input::new(&check).control()), cx), fresh_row];
        team.extend(rules);
        dialog::scroll(
            "settings-general",
            vec![
                group(None, vec![row("名称", None, div().w(px(280.)).child(Input::new(&title).control()), cx), row("目录", None, value_text(cwd, cx), cx)], cx),
                group(None, team, cx),
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
            owner: bool,
            running: bool,
            codex: bool,
            /// Its verdict is on work done by the same models.
            same_source: bool,
        }
        // As the form has them now: picking another model moves the mark at once.
        let sources: Vec<(String, String)> = f.members.iter().zip(&f.value.members).map(|(m, v)| {
            let (runtime, model) = m.choice(cx);
            (v.id.clone(), lila_core::config::model_source(runtime, &model))
        }).collect();
        let same: Vec<String> = lila_core::flow::self_verified(&f.value.flows, |by| sources.iter().find(|(id, _)| id == by).map(|(_, s)| s.clone())).into_iter().map(|(verifier, _)| verifier).collect();
        let items: Vec<Item> = f
            .members
            .iter()
            .zip(&f.value.members)
            .map(|(m, v)| {
                Item {
                    id: v.id.clone(),
                    count: v.count,
                    role: m.base.role_name.clone(),
                    owner: v.ids().contains(&owner),
                    running: v.ids().iter().any(|i| running.contains(i)),
                    codex: m.choice(cx).0 == Runtime::Codex,
                    same_source: same.contains(&v.id),
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
                        .h(px(52.))
                        .px_2()
                        .gap(px(10.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .when(active, |d| d.bg(crate::style::accent()))
                        .when(!active, |d| d.hover(|d| d.bg(crate::style::hover())))
                        .child(crate::style::cli_tile(if it.codex { "codex" } else { "claude" }, 22.))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(h_flex().gap_1().child(div().text_size(px(13.)).font_medium().truncate().child(if it.id.is_empty() { "（未命名）".to_owned() } else { it.id.clone() })).when(it.count > 1, |d| d.child(div().text_xs().text_color(theme.muted_foreground).child(format!("×{}", it.count)))))
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .min_w_0()
                                        .child(div().text_xs().text_color(theme.muted_foreground).truncate().child(it.role.clone()))
                                        .when(it.same_source, |d| d.child(div().flex_none().text_xs().text_color(crate::style::yellow()).child("同源验证"))),
                                ),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(f) = this.settings.as_mut() {
                                f.selected = ix;
                            }
                            cx.notify();
                        }))
                })),
            )
            .child(
                div().p_2().border_t_1().border_color(theme.border).child(Button::new("add-member").ghost().small().small_button().w_full().icon(IconName::Plus).label("添加成员").dropdown_menu(move |mut menu, _, _| {
                    for (id, name) in &roles {
                        let (weak, id) = (weak.clone(), id.clone());
                        menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, window, cx| {
                            let _ = weak.update(cx, |this, cx| this.add_member(&id, window, cx));
                        }));
                    }
                    menu
                })),
            );

        let editor = match items.get(selected) {
            Some(it) => self.member_editor(selected, it.owner, it.running, cx),
            None => div().into_any_element(),
        };
        h_flex().size_full().child(list).child(v_flex().flex_1().min_w_0().h_full().child(editor)).into_any_element()
    }

    fn member_editor(&mut self, ix: usize, is_owner: bool, is_running: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(m) = self.settings.as_ref().and_then(|f| f.members.get(ix)) else { return div().into_any_element() };
        let (base, effort, select, prompt, id_input) = (m.base.clone(), m.effort.clone(), m.model.clone(), m.prompt.clone(), m.id.clone());
        let (runtime, _) = m.choice(cx);
        let own_prompt = prompt.read(cx).value().trim() != base.role_prompt.trim();
        let (shown_id, count) = self.settings.as_ref().and_then(|f| f.value.members.get(ix)).map(|v| (v.id.clone(), v.count)).unwrap_or_default();

        let header = h_flex()
            .gap_3()
            .child(crate::style::cli_tile(if runtime == Runtime::Codex { "codex" } else { "claude" }, 36.))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_base().font_semibold().child(shown_id.clone()))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(base.role_name.clone())),
            )
            .when(!is_owner, |d| {
                d.child(Button::new(SharedString::from(format!("remove-{ix}"))).outline().small().small_button().text_color(crate::style::red()).label("移出团队").tooltip(if is_running { "保存后停掉它的终端，没做完的任务会取消" } else { "保存后从项目里去掉" }).on_click(cx.listener(move |this, _, _, cx| this.remove_member(ix, cx))))
            });

        let mut rows = Vec::new();
        if let Some(input) = &id_input {
            rows.push(row("编号", None, div().w(px(280.)).child(Input::new(input).control()), cx));
        }
        rows.push(row("模型", None, model_control(&select), cx));
        rows.push(row(
            "努力程度",
            None,
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
        if !is_owner {
            let down: Option<Act> = (count > 1).then(|| Box::new(move |this: &mut LilaApp, cx: &mut Context<LilaApp>| this.settings_edit(cx, |v| v.set_count(ix, count - 1))) as Act);
            let up: Option<Act> = (count < 9).then(|| Box::new(move |this: &mut LilaApp, cx: &mut Context<LilaApp>| this.settings_edit(cx, |v| v.set_count(ix, count + 1))) as Act);
            rows.push(row("同时工作", None, stepper(&format!("count-{ix}"), format!("{} 个", count.max(1)), down, up, cx), cx));
        }
        let prompt_row = block(
            "提示词",
            None,
            Textarea::new(&prompt).area(),
            own_prompt.then(|| Button::new(SharedString::from(format!("prompt-reset-{ix}"))).ghost().xsmall().icon(IconName::Undo2).label("恢复默认").on_click(cx.listener(move |this, _, window, cx| this.reset_prompt(ix, window, cx))).into_any_element()),
            cx,
        );
        dialog::scroll(SharedString::from(format!("member-editor-{ix}")), vec![header.into_any_element(), group(None, rows, cx), group(None, vec![prompt_row], cx)])
    }

    fn rule_row(&mut self, rule: Rule, now: &Settings, cx: &mut Context<Self>) -> AnyElement {
        let (title, unit) = rule.text();
        let v = rule.get(now);
        let (lo, hi) = rule.range();
        let down: Option<Act> = (v > lo).then(|| Box::new(move |this: &mut LilaApp, cx: &mut Context<LilaApp>| this.settings_edit(cx, |s| rule.set(&mut s.settings, v - 1))) as Act);
        let up: Option<Act> = (v < hi).then(|| Box::new(move |this: &mut LilaApp, cx: &mut Context<LilaApp>| this.settings_edit(cx, |s| rule.set(&mut s.settings, v + 1))) as Act);
        row(title, None, stepper(&format!("{rule:?}"), format!("{v} {unit}"), down, up, cx), cx)
    }

    pub fn settings_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.settings.as_ref()?;
        self.sync_ids(cx);
        let theme = cx.theme().clone();
        let running: Vec<String> = self.members().iter().filter(|m| m["status"] != "gone").map(|m| s(m, "id")).collect();
        let f = self.settings.as_ref()?;
        let (error, saving, page) = (f.error.clone(), f.saving, f.page);
        let kept = f.value.member_ids();
        let stopping: Vec<String> = f.base.member_ids().into_iter().filter(|m| !kept.contains(m) && running.contains(m)).collect();

        let (title, body) = match page {
            Page::General => ("通用", self.general_page(cx)),
            Page::Members => ("成员", self.members_page(&running, cx)),
        };
        let note = if let Some(e) = error {
            Some(h_flex().gap_2().text_color(theme.red).child(Icon::new(IconName::CircleAlert).small()).child(div().truncate().child(e)).into_any_element())
        } else if !stopping.is_empty() {
            Some(h_flex().gap_2().text_color(theme.yellow).child(Icon::new(IconName::TriangleAlert).small()).child(div().truncate().child(format!("保存后会停掉 {}，没做完的任务会取消。", stopping.join("、")))).into_any_element())
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
            (Nav { id: "nav-general", icon: IconName::Settings2, label: "通用", active: page == Page::General }, go(Page::General)),
            (Nav { id: "nav-members", icon: IconName::Bot, label: "成员", active: page == Page::Members }, go(Page::Members)),
        ];
        Some(self.dialog(
            Shell {
                id: "project-settings",
                nav,
                title: title.into(),
                body,
                note,
                saving,
                save_label: if saving { "保存中…" } else { "保存" },
                can_save: true,
                on_save: Box::new(|this, cx| this.save_settings(cx)),
                on_close: Box::new(|this, _| this.settings = None),
                footer: true,
            },
            cx,
        ))
    }
}
