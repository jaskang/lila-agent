//! Global settings, a dialog: the role library (every project's members start
//! from a role), usage per account, and where Lila keeps things. Roles are saved with
//! `lila::roles`; every project lilad runs reads them again.

use crate::app::LilaApp;
use crate::data;
use crate::dialog::{self, Act, Nav, Shell, block, group, row, value_text};
use crate::settings::{ModelSelect, effort_control, model_control, model_select, selected_model};
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::checkbox::Checkbox;
use gpui::component::input::{Input, InputState, Textarea, TextareaState};
use gpui::component::menu::{DropdownMenu, PopupMenuItem};
use gpui::component::scroll::ScrollableElement;
use gpui::component::{ActiveTheme, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
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
    Usage,
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
    /// The final prompt shown in place of the editor, while on.
    pub preview: Option<Preview>,
}

/// What a member using a role is started with, in one project (`lila::roles::preview`).
pub struct Preview {
    /// The role's index in `Prefs::roles`.
    pub role: usize,
    pub project: String,
    /// The member it was made for and its prompt, or why it can't be made.
    pub text: Result<(String, String), String>,
}

fn role_form(r: &RoleEdit, new: bool, window: &mut Window, cx: &mut Context<LilaApp>) -> RoleForm {
    RoleForm {
        base: (!new).then(|| r.clone()),
        id: r.id.clone(),
        effort: r.effort.clone(),
        can_assign: r.can_assign.clone(),
        name: cx.new(|cx| InputState::new(window, cx).default_value(r.name.clone())),
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
        self.prefs = Some(Prefs { page, roles: forms, deleted: Vec::new(), selected: 0, users, error: None, saving: false, preview: None });
        cx.notify();
    }

    /// Shows role `ix`'s final prompt in `project` (with its unsaved edits), or the editor again (`None`).
    fn preview_role(&mut self, ix: usize, project: Option<String>, cx: &mut Context<Self>) {
        let roles_dir = self.paths().roles();
        let cwd = project.as_ref().and_then(|id| self.snap.projects.iter().find(|p| p.id == *id)).map(|p| p.cwd.clone());
        let Some(p) = self.prefs.as_mut() else { return };
        p.preview = match (project, cwd, p.roles.get(ix)) {
            (Some(project), Some(cwd), Some(r)) => {
                let v = r.value(cx);
                let text = lila::roles::preview(&roles_dir, std::path::Path::new(&cwd), &v.id, &v.prompt).map_err(|e| format!("{e:#}"));
                Some(Preview { role: ix, project, text })
            }
            _ => None,
        };
        cx.notify();
    }

    /// Screenshots: the role library on `role`, its final prompt in the first project using it.
    #[cfg(feature = "shots")]
    pub fn dev_preview(&mut self, role: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_prefs(PrefPage::Roles, window, cx);
        let Some(p) = self.prefs.as_mut() else { return };
        let Some(ix) = p.roles.iter().position(|r| r.id == role) else { return };
        p.selected = ix;
        let first = p.users.get(role).and_then(|u| u.first().cloned());
        self.preview_role(ix, first, cx);
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
        let items: Vec<(String, String)> = p.roles.iter().map(|r| (r.value(cx).id, r.name.read(cx).value().to_string())).collect();
        let templates: Vec<(usize, String)> = items.iter().enumerate().map(|(i, (_, name))| (i, name.clone())).collect();
        let weak = cx.entity().downgrade();
        let list = v_flex()
            .w(px(200.))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(theme.border)
            .child(v_flex().id("role-list").flex_1().min_h_0().overflow_y_scrollbar().p_2().gap_0p5().children(items.iter().enumerate().map(|(ix, (id, name))| {
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
                    .child(v_flex().flex_1().min_w_0().child(div().text_size(px(13.)).font_medium().truncate().child(name.clone())).child(div().text_xs().text_color(theme.muted_foreground).truncate().child(id.clone())))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(p) = this.prefs.as_mut() {
                            p.selected = ix;
                        }
                        cx.notify();
                    }))
            })))
            .child(div().p_2().border_t_1().border_color(theme.border).child(Button::new("add-role").ghost().small().small_button().w_full().icon(IconName::Plus).label("新建角色").dropdown_menu(move |mut menu, _, _| {
                for (ix, name) in &templates {
                    let (weak, ix) = (weak.clone(), *ix);
                    menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, window, cx| {
                        let _ = weak.update(cx, |this, cx| this.new_role(ix, window, cx));
                    }));
                }
                menu
            })));
        let editor = if items.is_empty() { div().into_any_element() } else { self.role_editor(selected, cx) };
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

        let mut rows = Vec::new();
        if let Some(input) = &id_input {
            rows.push(row("编号", None, div().w(px(280.)).child(Input::new(input).control()), cx));
        }
        rows.push(row("名称", None, div().w(px(280.)).child(Input::new(&name_input).control()), cx));
        rows.push(row("模型", None, model_control(&select), cx));
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
            Checkbox::new(SharedString::from(format!("role-assign-{ix}-{id}"))).label(name).checked(on).on_click(cx.listener(move |this, on: &bool, _, cx| {
                if let Some(r) = this.pref_role(ix) {
                    r.can_assign.retain(|a| *a != target);
                    if *on {
                        r.can_assign.push(target.clone());
                    }
                }
                cx.notify();
            }))
        }));
        let mut body = vec![group(None, rows, cx)];
        // The PM may hand work to anyone.
        if !is_pm {
            body.push(group(None, vec![block("可以派给", None, assign, None, cx)], cx));
        }
        // Edit, or see what a member is started with: this prompt plus Lila's part.
        let shown = p.preview.as_ref().filter(|pv| pv.role == ix).map(|pv| (pv.project.clone(), pv.text.clone()));
        let first = used.first().cloned();
        let toggle = (!used.is_empty()).then(|| {
            dialog::segmented(&format!("prompt-view-{ix}"), &["编辑", "预览"], shown.is_some() as usize, move |this, i, _, cx| this.preview_role(ix, if i == 1 { first.clone() } else { None }, cx), cx)
        });
        let prompt_block = match shown {
            None => block("提示词", None, Textarea::new(&prompt).area(), toggle, cx),
            Some((project, text)) => {
                let weak = cx.entity().downgrade();
                let picker = Button::new(SharedString::from(format!("prompt-project-{ix}"))).outline().small().small_button().label(project.clone()).dropdown_caret(true).dropdown_menu({
                    let used = used.clone();
                    move |mut menu, _, _| {
                        for id in &used {
                            let (weak, id) = (weak.clone(), id.clone());
                            menu = menu.item(PopupMenuItem::new(id.clone()).on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |this, cx| this.preview_role(ix, Some(id.clone()), cx));
                            }));
                        }
                        menu
                    }
                });
                let content = match text {
                    Ok((_, text)) => {
                        let copy = text.clone();
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .id(SharedString::from(format!("prompt-preview-{ix}")))
                                    .max_h(px(320.))
                                    .overflow_y_scrollbar()
                                    .p_3()
                                    .rounded(px(8.))
                                    .bg(crate::style::well())
                                    .font_family(lila_term::theme::FONT)
                                    .text_size(px(11.5))
                                    .line_height(relative(1.6))
                                    .child(text),
                            )
                            .child(
                                h_flex()
                                    .child(picker)
                                    .child(div().flex_1())
                                    .child(Button::new(SharedString::from(format!("prompt-copy-{ix}"))).ghost().small().small_button().icon(IconName::Copy).label("复制").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone())))),
                            )
                            .into_any_element()
                    }
                    Err(e) => div().text_xs().text_color(theme.red).child(format!("生成不了：{e}")).into_any_element(),
                };
                block("提示词", None, content, toggle, cx)
            }
        };
        body.push(group(None, vec![prompt_block], cx));
        // A role in use, or the PM's, can't go: no button then.
        if !is_pm && used.is_empty() {
            body.push(h_flex().child(Button::new(SharedString::from(format!("role-delete-{ix}"))).outline().small().small_button().text_color(crate::style::red()).label("删除角色").on_click(cx.listener(move |this, _, _, cx| this.delete_role(ix, cx)))).into_any_element());
        }
        dialog::scroll(SharedString::from(format!("role-editor-{ix}")), body)
    }

    fn about_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let paths = self.paths().clone();
        let tilde = |p: std::path::PathBuf| data::tail(&data::tilde(&p.to_string_lossy()), 44);
        let daemon = if self.snap.daemon { "运行中" } else { "没有运行" };
        dialog::scroll(
            "prefs-about",
            vec![
                group(None, vec![row("版本", None, value_text(env!("CARGO_PKG_VERSION"), cx), cx), row("lilad", None, value_text(daemon, cx), cx)], cx),
                group(None, vec![row("角色", None, value_text(tilde(paths.roles()), cx), cx), row("数据库", None, value_text(tilde(paths.db()), cx), cx), row("日志", None, value_text(tilde(paths.state.clone()), cx), cx)], cx),
            ],
        )
    }

    pub fn prefs_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.prefs.as_ref()?;
        let theme = cx.theme().clone();
        let (page, error, saving) = (p.page, p.error.clone(), p.saving);
        let (title, body) = match page {
            PrefPage::Roles => ("角色", self.roles_page(cx)),
            PrefPage::Usage => ("用量", self.usage_page(cx)),
            PrefPage::About => ("关于", self.about_page(cx)),
        };
        let note = error.map(|e| h_flex().gap_2().text_color(theme.red).child(Icon::new(IconName::CircleAlert).small()).child(div().truncate().child(e)).into_any_element());
        let go = |pg: PrefPage| -> Act {
            Box::new(move |this: &mut LilaApp, _| {
                if let Some(p) = this.prefs.as_mut() {
                    p.page = pg;
                }
            })
        };
        let nav = vec![
            (Nav { id: "pref-nav-roles", icon: IconName::User, label: "角色", active: page == PrefPage::Roles }, go(PrefPage::Roles)),
            (Nav { id: "pref-nav-usage", icon: IconName::ChartPie, label: "用量", active: page == PrefPage::Usage }, go(PrefPage::Usage)),
            (Nav { id: "pref-nav-about", icon: IconName::Info, label: "关于", active: page == PrefPage::About }, go(PrefPage::About)),
        ];
        Some(self.dialog(
            Shell {
                id: "prefs",
                nav,
                title: title.into(),
                body,
                note,
                saving,
                save_label: if saving { "保存中…" } else { "保存" },
                can_save: true,
                on_save: Box::new(|this, cx| this.save_prefs(cx)),
                on_close: Box::new(|this, _| this.prefs = None),
                footer: page == PrefPage::Roles,
            },
            cx,
        ))
    }
}
