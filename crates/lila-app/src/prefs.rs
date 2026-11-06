//! Global settings, a dialog: the role library (every project's members start
//! from a role) and where Lila keeps things. Roles are saved with
//! `lila::roles`; every project lilad runs reads them again.

use crate::app::LilaApp;
use crate::data;
use crate::dialog::{self, Act, Nav, Shell, block, group, row, value_text};
use crate::settings::{ModelSelect, effort_control, model_control, model_select, notice, selected_model};
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::checkbox::Checkbox;
use gpui::component::input::{Input, InputState, Textarea, TextareaState};
use gpui::component::menu::{DropdownMenu, PopupMenuItem};
use gpui::component::scroll::ScrollableElement;
use gpui::component::tag::Tag;
use gpui::component::{ActiveTheme, Disableable, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use crate::style::Control as _;
use gpui::prelude::*;
use gpui::*;
use lila::roles::RoleEdit;
use lila_core::Runtime;
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefPage {
    Roles,
    About,
}

pub struct RoleForm {
    /// As loaded (`None`: added in this dialog).
    pub base: Option<RoleEdit>,
    pub id: String,
    pub effort: Option<String>,
    pub can_assign: Vec<String>,
    pub name: Entity<InputState>,
    /// The model, and so the CLI.
    pub model: ModelSelect,
    pub prompt: Entity<TextareaState>,
    /// Added in this dialog: its id can still be typed.
    pub id_input: Option<Entity<InputState>>,
}

pub struct Prefs {
    pub page: PrefPage,
    pub roles: Vec<RoleForm>,
    /// Loaded roles removed in this dialog, deleted on save.
    pub deleted: Vec<String>,
    pub selected: usize,
    /// Role id → projects using it (those can't be deleted).
    pub users: BTreeMap<String, Vec<String>>,
    pub error: Option<String>,
    pub saving: bool,
}

fn role_form(r: &RoleEdit, new: bool, window: &mut Window, cx: &mut Context<LilaApp>) -> RoleForm {
    RoleForm {
        base: (!new).then(|| r.clone()),
        id: r.id.clone(),
        effort: r.effort.clone(),
        can_assign: r.can_assign.clone(),
        name: cx.new(|cx| InputState::new(window, cx).placeholder("显示名，比如 工程师").default_value(r.name.clone())),
        model: model_select(&r.model, r.runtime, window, cx),
        prompt: cx.new(|cx| TextareaState::new(window, cx).rows(14).default_value(r.prompt.clone())),
        id_input: new.then(|| cx.new(|cx| InputState::new(window, cx).placeholder("角色编号").default_value(r.id.clone()))),
    }
}

impl RoleForm {
    fn value(&self, cx: &App) -> RoleEdit {
        let fallback = self.base.as_ref().map(|b| (b.runtime, b.model.clone())).unwrap_or((Runtime::Claude, String::new()));
        let (runtime, model) = selected_model(&self.model, cx).unwrap_or(fallback);
        RoleEdit {
            id: self.id_input.as_ref().map(|i| i.read(cx).value().trim().to_owned()).unwrap_or_else(|| self.id.clone()),
            name: self.name.read(cx).value().trim().to_owned(),
            runtime,
            model,
            effort: self.effort.clone(),
            can_assign: self.can_assign.clone(),
            prompt: self.prompt.read(cx).value().to_string(),
        }
    }

    fn changed(&self, cx: &App) -> bool {
        self.base.as_ref() != Some(&self.value(cx))
    }
}

impl LilaApp {
    pub fn open_prefs(&mut self, page: PrefPage, window: &mut Window, cx: &mut Context<Self>) {
        let roles = match lila::roles::load(&self.paths().roles()) {
            Ok(r) => r,
            Err(e) => {
                self.notice = Some(format!("读取角色失败：{e:#}"));
                cx.notify();
                return;
            }
        };
        let projects: Vec<(String, std::path::PathBuf)> = self.snap.projects.iter().map(|p| (p.id.clone(), p.cwd.clone().into())).collect();
        let users = lila::roles::users(&projects);
        // The PM first, then the others as the defaults list them.
        let order = |id: &str| lila::roles::DEFAULTS.iter().position(|(d, _, _)| *d == id).unwrap_or(usize::MAX);
        let mut list: Vec<RoleEdit> = roles.values().map(RoleEdit::of).collect();
        list.sort_by_key(|r| (order(&r.id), r.id.clone()));
        let forms = list.iter().map(|r| role_form(r, false, window, cx)).collect();
        self.settings = None;
        self.draft = None;
        self.dialog_offset = Point::default();
        self.prefs = Some(Prefs { page, roles: forms, deleted: Vec::new(), selected: 0, users, error: None, saving: false });
        cx.notify();
    }

    fn pref_role(&mut self, ix: usize) -> Option<&mut RoleForm> {
        self.prefs.as_mut().and_then(|p| p.roles.get_mut(ix))
    }

    fn new_role(&mut self, from: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.prefs.as_ref() else { return };
        let Some(src) = p.roles.get(from).map(|r| r.value(cx)) else { return };
        let taken: Vec<String> = p.roles.iter().map(|r| r.value(cx).id).collect();
        let id = (2..).map(|n| format!("{}{n}", src.id)).find(|id| !taken.contains(id)).expect("a free id");
        let r = RoleEdit { id, name: format!("{}（副本）", src.name), ..src };
        // A copy is kept as its own new role, with the template's model chosen.
        let form = role_form(&r, true, window, cx);
        if let Some(p) = self.prefs.as_mut() {
            p.roles.push(form);
            p.selected = p.roles.len() - 1;
        }
        cx.notify();
    }

    fn delete_role(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(p) = self.prefs.as_mut() else { return };
        if ix >= p.roles.len() {
            return;
        }
        let r = p.roles.remove(ix);
        if r.base.is_some() {
            p.deleted.push(r.id.clone());
        }
        for other in &mut p.roles {
            other.can_assign.retain(|a| *a != r.id);
        }
        p.selected = p.selected.min(p.roles.len().saturating_sub(1));
        cx.notify();
    }

    fn save_prefs(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.prefs.as_mut() else { return };
        p.saving = true;
        p.error = None;
        let p = self.prefs.as_ref().expect("open");
        let changed: Vec<RoleEdit> = p.roles.iter().filter(|r| r.changed(cx)).map(|r| r.value(cx)).collect();
        let deleted = p.deleted.clone();
        let users = p.users.clone();
        let paths = self.paths().clone();
        let projects: Vec<data::Project> = if self.snap.daemon { self.snap.projects.clone() } else { Vec::new() };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move {
                    let dir = paths.roles();
                    let mut ids: Vec<&str> = changed.iter().map(|r| r.id.as_str()).collect();
                    ids.sort();
                    if ids.windows(2).any(|w| w[0] == w[1]) {
                        anyhow::bail!("两个角色用了同一个编号");
                    }
                    for id in &deleted {
                        lila::roles::delete(&dir, id, &users)?;
                    }
                    for r in &changed {
                        lila::roles::save(&dir, r)?;
                    }
                    // Members on old role settings are marked until restarted. A project
                    // without its .lila/project.toml was never loaded: nothing to reload.
                    let failed: Vec<String> = projects
                        .iter()
                        .filter(|p| std::path::Path::new(&p.cwd).join(".lila/project.toml").exists())
                        .filter_map(|p| data::act(&paths, p, "project.reload", json!({}), false).err().map(|e| format!("{}（{e:#}）", p.label())))
                        .collect();
                    anyhow::Ok((!failed.is_empty()).then(|| format!("角色已保存，但这些项目没能重新读取：{}。重启 lilad 后生效。", failed.join("；"))))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(warn) => {
                        this.prefs = None;
                        this.notice = warn;
                    }
                    Err(e) => {
                        if let Some(p) = &mut this.prefs {
                            p.saving = false;
                            p.error = Some(format!("{e:#}"));
                        }
                    }
                }
                this.refresh_now();
                cx.notify();
            });
        })
        .detach();
    }

    fn roles_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(p) = self.prefs.as_ref() else { return div().into_any_element() };
        let selected = p.selected.min(p.roles.len().saturating_sub(1));
        let items: Vec<(String, String, bool, bool)> = p.roles.iter().map(|r| (r.value(cx).id, r.name.read(cx).value().to_string(), r.base.is_none(), r.changed(cx))).collect();
        let templates: Vec<(usize, String)> = items.iter().enumerate().map(|(i, (_, name, _, _))| (i, name.clone())).collect();
        let weak = cx.entity().downgrade();
        let list = v_flex()
            .w(px(200.))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(theme.border)
            .child(v_flex().id("role-list").flex_1().min_h_0().overflow_y_scrollbar().p_2().gap_0p5().children(items.iter().enumerate().map(|(ix, (id, name, new, changed))| {
                let active = ix == selected;
                h_flex()
                    .id(SharedString::from(format!("role-item-{ix}")))
                    .h(px(52.))
                    .px_2()
                    .gap(px(10.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(active, |d| d.bg(crate::style::accent()))
                    .when(!active, |d| d.hover(|d| d.bg(crate::style::hover())))
                    .child(Icon::new(IconName::User).small().text_color(theme.muted_foreground))
                    .child(v_flex().flex_1().min_w_0().child(div().text_size(px(13.)).font_medium().truncate().child(name.clone())).child(div().text_xs().text_color(theme.muted_foreground).truncate().child(id.clone())))
                    .when(*new, |d| d.child(Tag::success().xsmall().child("新")))
                    .when(*changed && !*new, |d| d.child(div().size(px(6.)).rounded_full().bg(theme.blue)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(p) = this.prefs.as_mut() {
                            p.selected = ix;
                        }
                        cx.notify();
                    }))
            })))
            .child(div().p_2().border_t_1().border_color(theme.border).child(Button::new("add-role").ghost().small().small_button().w_full().icon(IconName::Plus).label("新建角色").dropdown_menu(move |mut menu, _, _| {
                menu = menu.label("以这个角色为模板");
                for (ix, name) in &templates {
                    let (weak, ix) = (weak.clone(), *ix);
                    menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, window, cx| {
                        let _ = weak.update(cx, |this, cx| this.new_role(ix, window, cx));
                    }));
                }
                menu
            })));
        let editor = if items.is_empty() { div().p_6().text_color(theme.muted_foreground).child("还没有角色。").into_any_element() } else { self.role_editor(selected, cx) };
        h_flex().size_full().child(list).child(v_flex().flex_1().min_w_0().h_full().child(editor)).into_any_element()
    }

    fn role_editor(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(p) = self.prefs.as_ref() else { return div().into_any_element() };
        let Some(r) = p.roles.get(ix) else { return div().into_any_element() };
        let v = r.value(cx);
        let (name_input, select, prompt, id_input, effort, can_assign) = (r.name.clone(), r.model.clone(), r.prompt.clone(), r.id_input.clone(), r.effort.clone(), r.can_assign.clone());
        let is_pm = v.id == lila::roles::PM_ROLE;
        let used = p.users.get(&r.id).cloned().filter(|_| r.base.is_some()).unwrap_or_default();
        let others: Vec<(String, String)> = p.roles.iter().enumerate().filter(|(i, _)| *i != ix).map(|(_, o)| (o.value(cx).id, o.name.read(cx).value().to_string())).collect();
        let is_default = lila::roles::DEFAULTS.iter().any(|(d, _, _)| *d == v.id);

        let header = h_flex()
            .gap_3()
            .child(div().size(px(36.)).flex_none().rounded(px(10.)).bg(crate::style::fill()).border_1().border_color(crate::style::hairline()).flex().items_center().justify_center().child(Icon::new(IconName::User).text_color(crate::style::muted())))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_base().font_semibold().child(if v.name.is_empty() { v.id.clone() } else { v.name.clone() }))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(if used.is_empty() { format!("{} · 没有项目在用", v.id) } else { format!("{} · 用在 {}", v.id, used.join("、")) })),
            )
            .child(
                Button::new(SharedString::from(format!("role-delete-{ix}")))
                    .outline()
                    .small()
                    .label("删除角色")
                    .disabled(is_pm || !used.is_empty())
                    .tooltip(if is_pm {
                        "每个项目都有且只有一个 PM，这个角色不能删除".to_owned()
                    } else if used.is_empty() {
                        "保存后删除它的文件".to_owned()
                    } else {
                        format!("{} 在用它，先把那些成员换成别的角色", used.join("、"))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.delete_role(ix, cx))),
            );

        let mut rows = Vec::new();
        if let Some(input) = &id_input {
            rows.push(row("角色编号", Some("项目配置里写的 role".into()), div().w(px(280.)).child(Input::new(input).control()), cx));
        }
        rows.push(row("名称", Some("界面里显示的名字。".into()), div().w(px(280.)).child(Input::new(&name_input).control()), cx));
        let cli = match v.runtime {
            Runtime::Claude => "Claude Code",
            Runtime::Codex => "Codex（ChatGPT）",
        };
        rows.push(row("模型", Some(cli.into()), model_control(&select), cx));
        rows.push(row(
            "努力程度",
            None,
            effort_control(
                format!("role-effort-{ix}"),
                effort.as_deref(),
                move |this, e| {
                    if let Some(r) = this.pref_role(ix) {
                        r.effort = e;
                    }
                },
                cx,
            ),
            cx,
        ));
        let assign = h_flex().gap_4().flex_wrap().children(others.into_iter().map(|(id, name)| {
            let on = can_assign.contains(&id);
            let target = id.clone();
            Checkbox::new(SharedString::from(format!("role-assign-{ix}-{id}"))).label(format!("{name}（{id}）")).checked(on).on_click(cx.listener(move |this, on: &bool, _, cx| {
                if let Some(r) = this.pref_role(ix) {
                    r.can_assign.retain(|a| *a != target);
                    if *on {
                        r.can_assign.push(target.clone());
                    }
                }
                cx.notify();
            }))
        }));
        let assign_row = block("默认可以派给", Some("项目里没单独设置指派规则时，这个角色的成员可以把任务派给哪些角色的成员。PM 总是可以派给所有人。".into()), assign, None, cx);

        let mut body = vec![header.into_any_element(), group(Some("运行"), rows, cx)];
        if is_pm {
            body.push(notice(IconName::Info, theme.muted_foreground, "每个项目都有且只有一个 PM：它和你对话，可以把任务派给任何人。", cx));
        } else {
            body.push(group(Some("协作"), vec![assign_row], cx));
        }
        body.extend([group(Some("提示词"), vec![block("预设提示词", Some("每个用这个角色、又没有自己提示词的成员都会用它。Lila 会在后面接上协作协议。".into()), Textarea::new(&prompt).area(), None, cx)], cx)]);
        if is_default {
            body.push(notice(IconName::Info, theme.muted_foreground, "这是 Lila 自带的角色。改动保存在你的角色目录里，Lila 升级不会覆盖。", cx));
        }
        if !used.is_empty() {
            body.push(notice(IconName::Info, theme.yellow, "保存后，正在运行、用这个角色的成员会标「待重启」，重启后才用上新设置。", cx));
        }
        dialog::scroll(SharedString::from(format!("role-editor-{ix}")), body)
    }

    fn about_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let paths = self.paths().clone();
        let tilde = |p: std::path::PathBuf| data::tilde(&p.to_string_lossy());
        let socket = paths.socket().map(tilde).unwrap_or_default();
        let daemon = if self.snap.daemon { "运行中" } else { "没有运行" };
        dialog::scroll(
            "prefs-about",
            vec![
                group(
                    Some("Lila"),
                    vec![
                        row("版本", None, value_text(env!("CARGO_PKG_VERSION"), cx), cx),
                        row("lilad", Some("唯一写状态的后台进程；团队启动时自动启动。".into()), value_text(daemon, cx), cx),
                        row("项目数", None, value_text(self.snap.projects.len().to_string(), cx), cx),
                    ],
                    cx,
                ),
                group(
                    Some("文件位置"),
                    vec![
                        row("角色", Some("每个角色一个 .toml 和一个 .md 提示词。".into()), value_text(tilde(paths.roles()), cx), cx),
                        row("数据库", Some("任务、收件箱和动态。".into()), value_text(tilde(paths.db()), cx), cx),
                        row("状态目录", Some("lilad 的日志、成员提示词和窗口标签。".into()), value_text(tilde(paths.state.clone()), cx), cx),
                        row("socket", None, value_text(socket, cx), cx),
                    ],
                    cx,
                ),
            ],
        )
    }

    pub fn prefs_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.prefs.as_ref()?;
        let theme = cx.theme().clone();
        let (page, error, saving, count) = (p.page, p.error.clone(), p.saving, p.roles.len());
        let dirty = !p.deleted.is_empty() || p.roles.iter().any(|r| r.changed(cx));
        let (title, description, body) = match page {
            PrefPage::Roles => ("角色", "项目成员从角色开始：用哪个 CLI、模型、努力程度和提示词。项目设置里可以再按成员覆盖。", self.roles_page(cx)),
            PrefPage::About => ("关于", "Lila 的版本，以及它把东西放在哪里。", self.about_page(cx)),
        };
        let note = match error {
            Some(e) => Some(h_flex().gap_2().text_color(theme.red).child(Icon::new(IconName::CircleAlert).small()).child(div().truncate().child(e)).into_any_element()),
            None => dirty.then(|| div().text_color(theme.muted_foreground).child("有未保存的更改").into_any_element()),
        };
        let go = |pg: PrefPage| -> Act {
            Box::new(move |this: &mut LilaApp, _| {
                if let Some(p) = this.prefs.as_mut() {
                    p.page = pg;
                }
            })
        };
        let nav = vec![
            (Nav { id: "pref-nav-roles", icon: IconName::User, label: "角色", active: page == PrefPage::Roles, badge: Some(count.to_string()) }, go(PrefPage::Roles)),
            (Nav { id: "pref-nav-about", icon: IconName::Info, label: "关于", active: page == PrefPage::About, badge: None }, go(PrefPage::About)),
        ];
        let heading = v_flex().gap_1().child(div().text_xs().text_color(theme.muted_foreground).child("全局设置")).child(div().font_semibold().child("Lila")).into_any_element();
        Some(self.dialog(
            Shell {
                id: "prefs",
                heading,
                nav,
                title: title.into(),
                description: description.into(),
                body,
                note,
                saving,
                save_label: if saving { "保存中…" } else { "保存" },
                can_save: true,
                back: None,
                on_save: Box::new(|this, cx| this.save_prefs(cx)),
                on_close: Box::new(|this, _| this.prefs = None),
            },
            cx,
        ))
    }
}
