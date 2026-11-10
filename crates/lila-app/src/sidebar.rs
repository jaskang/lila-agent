//! The sidebar card: a new session, then each project with its sessions. A session's row says
//! how it is doing at a glance (docs: DESIGN.md, Sidebar); the right-click menus rename, end,
//! resume and archive.

use crate::app::{LilaApp, Rename};
use crate::data::{self, Project};
use crate::style::{self, card, muted, muted2};
use crate::ui::{BAR, TRAFFIC_LIGHTS, avatar, drag_bar, dot, s};
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui::component::scroll::ScrollableElement;
use gpui::component::spinner::Spinner;
use gpui::component::{Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use crate::style::Control as _;
use gpui::prelude::*;
use gpui::*;
use serde_json::{Value, json};
use std::time::Duration;

/// A project lists this many sessions (those that need the person, or run, always); the rest are one click away.
const SHOWN: usize = 8;

/// A section heading in the sidebar (项目).
fn label(text: impl Into<SharedString>) -> Div {
    div().text_xs().font_semibold().text_color(muted()).child(text.into())
}

/// A sidebar row: a soft highlight when chosen or hovered.
fn row(id: impl Into<ElementId>, active: bool) -> Stateful<Div> {
    h_flex()
        .id(id)
        .mx_2()
        .px_2()
        .h(px(style::ROW))
        .mb(px(style::ROW_GAP))
        .gap_2()
        .rounded(px(style::R_CTL))
        .cursor_pointer()
        .when(active, |d| d.bg(style::accent()))
        .when(!active, |d| d.hover(|d| d.bg(style::hover())))
}

/// A tile that breathes a red ring while its session waits for you: slow
/// enough to notice without nagging.
fn waiting_ring(tile: Div, id: String) -> AnyElement {
    tile.with_animation(SharedString::from(format!("ring-{id}")), Animation::new(Duration::from_millis(1800)).repeat(), |d, t| {
        let k = (t / 0.7).min(1.);
        let ease = 1. - (1. - k).powi(3);
        d.shadow(vec![BoxShadow { color: style::red().opacity(0.55 * (1. - k)), offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(5. * ease), inset: false }])
    })
    .into_any_element()
}

/// What a waiting session waits for, in the words of its second line.
pub fn waiting_words(t: &Value) -> &'static str {
    if t["waiting"] == "terminal" { "终端在等你" } else { "要你允许" }
}

/// Which of a project's threads the sidebar lists: the latest `SHOWN`, and always those that need the person,
/// run, are shown or archived. The rest count as `more`.
pub fn visible(threads: &[Value], shown: Option<&str>, all: bool) -> (Vec<Value>, usize) {
    let mut out = Vec::new();
    let mut taken = 0;
    for t in threads {
        // The archived ones are listed only when asked for, and then all of them.
        let live = matches!(t["state"].as_str(), Some("waiting" | "running" | "starting")) || t["archived"] == true;
        let keep = all || live || shown == t["id"].as_str() || taken < SHOWN;
        if keep {
            out.push(t.clone());
            if !live && shown != t["id"].as_str() {
                taken += 1;
            }
        }
    }
    let more = threads.len() - out.len();
    (out, more)
}

impl LilaApp {
    /// One thread row under its project: the engine's tile, the title, and on the right how it is doing.
    fn thread_row(&self, t: &Value, shown: bool, now: i64, cx: &mut Context<Self>) -> AnyElement {
        let id = s(t, "id");
        let state = s(t, "state");
        let waiting = state == "waiting";
        let ended = state == "ended";
        let archived = t["archived"] == true;
        let click = id.clone();
        let tile = style::cli_tile(&s(t, "engine"), 20.).when(ended || archived, |d| d.opacity(0.5));
        let tile = if waiting { waiting_ring(tile, id.clone()) } else { tile.into_any_element() };
        let mark = match state.as_str() {
            "running" | "starting" => Spinner::new().xsmall().color(style::blue()).into_any_element(),
            "waiting" => dot(style::red()).size(px(8.)).into_any_element(),
            _ => div().text_size(px(11.)).text_color(muted2()).child(data::age(t["active_ms"].as_i64().unwrap_or(0), now)).into_any_element(),
        };
        h_flex()
            .id(SharedString::from(format!("thread-{id}")))
            .relative()
            .ml(px(18.))
            .mr_2()
            .px_2()
            .h(px(if waiting { style::THREAD_ROW_WAITING } else { style::ROW }))
            .mb(px(style::ROW_GAP))
            .gap(px(10.))
            .rounded(px(style::R_CTL))
            .cursor_pointer()
            .when(waiting, |d| d.bg(style::red().opacity(0.1)).hover(|d| d.bg(style::red().opacity(0.15))))
            .when(shown, |d| d.bg(if waiting { style::red().opacity(0.2) } else { style::accent() }))
            .when(!shown && !waiting, |d| d.hover(|d| d.bg(style::hover())))
            .child(tile)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.))
                    .child(div().text_size(px(13.)).font_medium().truncate().text_color(if ended || archived { muted() } else { style::fg() }).child(s(t, "title")))
                    .when(waiting, |d| d.child(div().text_xs().truncate().text_color(style::tint(style::red(), 0.25)).child(waiting_words(t)))),
            )
            .child(div().flex_none().child(mark))
            .on_click(cx.listener(move |this, _, _, cx| this.show_thread(click.clone(), cx)))
            .context_menu(self.thread_menu(t, cx))
            .into_any_element()
    }

    /// The row of the session about to be made: while its box is open, it is where the person is.
    fn new_row(&self) -> AnyElement {
        h_flex()
            .id("new-row")
            .ml(px(18.))
            .mr_2()
            .px_2()
            .h(px(style::ROW))
            .mb(px(style::ROW_GAP))
            .gap(px(10.))
            .rounded(px(style::R_CTL))
            .bg(style::accent())
            .child(div().size(px(20.)).flex_none().flex().items_center().justify_center().child(Icon::new(IconName::Plus).size(px(14.)).text_color(muted())))
            .child(div().text_size(px(13.)).font_medium().text_color(muted()).child("新会话"))
            .into_any_element()
    }

    pub fn sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let quiet = style::quiet(cx);
        let shown = self.thread.clone();
        let projects = self.snap.projects.clone();
        let now = data::now_ms();
        let archived_total = self.snap.threads.iter().filter(|t| t["archived"] == true).count();
        let mut rows: Vec<AnyElement> = Vec::new();
        for p in &projects {
            let threads = self.threads_of(&p.id);
            let waiting = threads.iter().filter(|t| t["state"] == "waiting").count();
            let folded = self.folded.contains(&p.id);
            let (id, add, fold) = (p.id.clone(), p.id.clone(), p.id.clone());
            rows.push(
                row(SharedString::from(format!("project-{id}")), false)
                    .child(Icon::new(if folded { IconName::ChevronRight } else { IconName::ChevronDown }).size(px(12.)).text_color(muted2()))
                    .child(avatar(&p.id, 20., cx))
                    .child(div().text_size(px(13.)).font_semibold().truncate().min_w_0().child(p.label()))
                    // project.toml can't be read (the folder moved, the file is broken): the project runs on defaults.
                    .when_some(p.error.clone(), |d, e| {
                        d.child(div().id(SharedString::from(format!("project-error-{id}"))).flex_none().tooltip(move |window, cx| gpui::component::tooltip::Tooltip::new(e.clone()).build(window, cx)).child(Icon::new(IconName::TriangleAlert).size(px(13.)).text_color(style::yellow())))
                    })
                    .child(div().flex_1())
                    .when(waiting > 0, |d| d.child(style::pill(style::red(), waiting.to_string())))
                    .child(Button::new(SharedString::from(format!("new-thread-{id}"))).custom(quiet).xsmall().icon(IconName::Plus).tooltip("新会话 (⌘N)").on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.show_new(add.clone(), window, cx)
                    })))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.folded.remove(&fold) {
                            this.folded.insert(fold.clone());
                        }
                        cx.notify();
                    }))
                    .context_menu(self.project_menu(p, cx))
                    .into_any_element(),
            );
            if folded {
                continue;
            }
            if self.thread.is_none() && self.new_in.as_deref() == Some(p.id.as_str()) {
                rows.push(self.new_row());
            }
            let mut listed = threads;
            if self.show_archived {
                listed.extend(self.archived_of(&p.id));
            }
            let all = self.unfolded.contains(&p.id);
            let (seen, more) = visible(&listed, shown.as_deref(), all);
            rows.extend(seen.iter().map(|t| self.thread_row(t, shown.as_deref() == t["id"].as_str(), now, cx)));
            if more > 0 || (all && listed.len() > SHOWN) {
                let id = p.id.clone();
                rows.push(
                    h_flex()
                        .id(SharedString::from(format!("more-{id}")))
                        .ml(px(18.))
                        .mr_2()
                        .px_2()
                        .h(px(26.))
                        .gap(px(6.))
                        .rounded(px(style::R_CTL))
                        .cursor_pointer()
                        .text_xs()
                        .text_color(muted2())
                        .hover(|d| d.bg(style::hover()).text_color(muted()))
                        .child(Icon::new(if all { IconName::ChevronUp } else { IconName::ChevronDown }).size(px(12.)))
                        .child(if all { "收起".to_owned() } else { format!("更多 {more}") })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.unfolded.remove(&id) {
                                this.unfolded.insert(id.clone());
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
        }

        let current = self.current_project();
        let body = card()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            // The first row: the traffic lights, the app's name, Lila's settings. It drags the window.
            .child(
                drag_bar(h_flex().id("sidebar-top"), cx)
                    .h(px(BAR))
                    .flex_none()
                    .pl(px(TRAFFIC_LIGHTS))
                    .pr(px(10.))
                    .child(div().text_size(px(13.)).font_semibold().child("Lila"))
                    .child(div().flex_1())
                    .child(Button::new("prefs").custom(quiet).xsmall().icon(IconName::Settings).tooltip("设置 (⌘,)").on_click(cx.listener(|this, _, _, cx| this.open_prefs(crate::prefs::PrefPage::Usage, cx)))),
            )
            .children(current.map(|p| {
                row("new-session", false)
                    .mb(px(6.))
                    .gap_2()
                    .child(Icon::new(IconName::Plus).size(px(14.)).text_color(muted()))
                    .child(div().text_size(px(13.)).font_medium().child("新会话"))
                    .tooltip(|window, cx| gpui::component::tooltip::Tooltip::new("⌘N").build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| this.show_new(p.clone(), window, cx)))
            }))
            .child(
                h_flex()
                    .pl_4()
                    .pr(px(10.))
                    .py(px(6.))
                    .child(label("项目"))
                    .child(div().flex_1())
                    // Archived sessions are hidden; this brings them back into the list.
                    .when(archived_total > 0 || self.show_archived, |d| {
                        d.child(
                            Button::new("show-archived")
                                .custom(quiet)
                                .xsmall()
                                .icon(style::icon("lila/archive.svg").text_color(if self.show_archived { style::fg() } else { muted() }))
                                .when(self.show_archived, |b| b.bg(style::accent()))
                                .tooltip(format!("已归档 ({archived_total})"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_archived = !this.show_archived;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(Button::new("add-project").custom(quiet).xsmall().icon(IconName::Plus).tooltip("添加项目").on_click(cx.listener(|this, _, window, cx| this.choose_project(window, cx)))),
            )
            .child(v_flex().id("thread-tree").flex_1().min_h_0().overflow_y_scrollbar().pb_2().children(rows));

        v_flex().size_full().text_color(style::fg()).child(body)
    }

    /// Asks for a folder, and adds it as a project.
    pub fn choose_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("添加".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await
                && let Some(dir) = paths.into_iter().next()
            {
                let _ = this.update_in(cx, |this, window, cx| this.add_project(dir, window, cx));
            }
        })
        .detach();
    }

    /// The right-click menu of a project.
    fn project_menu(&self, p: &Project, cx: &mut Context<Self>) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let weak = cx.entity().downgrade();
        let (id, cwd) = (p.id.clone(), p.cwd.clone());
        move |menu, _, _| {
            let (w1, id1) = (weak.clone(), id.clone());
            let (w2, id2) = (weak.clone(), id.clone());
            let cwd = cwd.clone();
            menu.item(PopupMenuItem::new("新会话").icon(IconName::Plus).on_click(move |_, window, cx| {
                let _ = w1.update(cx, |this, cx| this.show_new(id1.clone(), window, cx));
            }))
            .item(PopupMenuItem::new("项目设置…").icon(IconName::Settings2).on_click(move |_, window, cx| {
                let _ = w2.update(cx, |this, cx| this.open_settings(id2.clone(), window, cx));
            }))
            .separator()
            .item(PopupMenuItem::new("在访达中显示").icon(IconName::FolderOpen).on_click(move |_, _, _| {
                let _ = std::process::Command::new("open").arg(&cwd).spawn();
            }))
            .separator()
            .item(PopupMenuItem::new("从列表移除…").icon(style::icon("lila/trash.svg")).on_click({
                let (weak, id) = (weak.clone(), id.clone());
                move |_, _, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.removing = this.snap.projects.iter().find(|p| p.id == id).cloned();
                        cx.notify();
                    });
                }
            }))
        }
    }

    /// The right-click menu of a session: rename, end or resume, archive or bring back.
    fn thread_menu(&self, t: &Value, cx: &mut Context<Self>) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let weak = cx.entity().downgrade();
        let (id, title) = (s(t, "id"), s(t, "title"));
        let ended = t["state"] == "ended";
        let archived = t["archived"] == true;
        move |menu, _, _| {
            let (w, id1, title1) = (weak.clone(), id.clone(), title.clone());
            let menu = menu.item(PopupMenuItem::new("重命名…").icon(style::icon("lila/pencil.svg")).on_click(move |_, window, cx| {
                let _ = w.update(cx, |this, cx| this.open_rename(id1.clone(), title1.clone(), window, cx));
            }));
            let (w, id2) = (weak.clone(), id.clone());
            let menu = if ended {
                menu.item(PopupMenuItem::new("继续").icon(IconName::Play).on_click(move |_, _, cx| {
                    let _ = w.update(cx, |this, cx| this.act("thread.resume", json!({"thread": id2}), cx));
                }))
            } else {
                menu.item(PopupMenuItem::new("结束").icon(style::icon("lila/power.svg")).on_click(move |_, _, cx| {
                    let _ = w.update(cx, |this, cx| this.act("thread.stop", json!({"thread": id2}), cx));
                }))
            };
            let (w, id3) = (weak.clone(), id.clone());
            if archived {
                menu.item(PopupMenuItem::new("取消归档").icon(style::icon("lila/archive-restore.svg")).on_click(move |_, _, cx| {
                    let _ = w.update(cx, |this, cx| this.act("thread.unarchive", json!({"thread": id3}), cx));
                }))
            } else {
                menu.item(PopupMenuItem::new("归档").icon(style::icon("lila/archive.svg")).on_click(move |_, _, cx| {
                    let _ = w.update(cx, |this, cx| this.act("thread.archive", json!({"thread": id3}), cx));
                }))
            }
        }
    }

    /// Opens the rename dialog on a session.
    pub fn open_rename(&mut self, thread: String, title: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.save_rename(cx);
            }
        })
        .detach();
        input.update(cx, |i, cx| i.focus(window, cx));
        self.renaming = Some(Rename { thread, input });
        cx.notify();
    }

    fn save_rename(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.renaming.take() else { return };
        let title = r.input.read(cx).value().trim().to_owned();
        if !title.is_empty() {
            self.act("thread.rename", json!({"thread": r.thread, "title": title}), cx);
        }
        cx.notify();
    }

    pub fn rename_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let quiet = style::quiet(cx);
        let r = self.renaming.as_ref()?;
        let input = Input::new(&r.input).control();
        Some(crate::dialog::small(
            "rename-dialog",
            "重命名".into(),
            None,
            Some(input.into_any_element()),
            vec![
                Button::new("rename-no").custom(quiet).label("取消").on_click(cx.listener(|this, _, _, cx| {
                    this.renaming = None;
                    cx.notify();
                }))
                .into_any_element(),
                Button::new("rename-yes").primary().label("保存").on_click(cx.listener(|this, _, _, cx| this.save_rename(cx))).into_any_element(),
            ],
            Box::new(|this, _| this.renaming = None),
            cx,
        ))
    }

    /// Asks before removing a project from the list: what goes, what stays.
    pub fn remove_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let quiet = style::quiet(cx);
        let p = self.removing.clone()?;
        let running = self.threads_of(&p.id).iter().any(|t| t["state"] != "ended");
        let text = format!("{}会话和对话记录会删掉，文件夹不动。", if running { "运行中的会话会先停止，" } else { "" });
        Some(crate::dialog::small(
            "remove-dialog",
            format!("从列表移除「{}」？", p.label()),
            Some(text),
            None,
            vec![
                Button::new("remove-no").custom(quiet).label("取消").on_click(cx.listener(|this, _, _, cx| {
                    this.removing = None;
                    cx.notify();
                }))
                .into_any_element(),
                Button::new("remove-yes").outline().text_color(style::red()).label("移除").on_click(cx.listener(move |this, _, _, cx| this.remove_project(p.clone(), cx))).into_any_element(),
            ],
            Box::new(|this, _| this.removing = None),
            cx,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::visible;
    use serde_json::{Value, json};

    fn threads(states: &[&str]) -> Vec<Value> {
        states.iter().enumerate().map(|(i, s)| json!({"id": format!("t{i}"), "state": s})).collect()
    }

    #[test]
    fn the_latest_few_show_and_those_that_need_you_always() {
        let mut states = vec!["idle"; 10];
        states[9] = "waiting";
        states[8] = "running";
        let list = threads(&states);
        let (shown, more) = visible(&list, None, false);
        // 8 plain ones fit, the waiting and the running one come in beyond them: 10 - 2 = 8 plain, and all fit.
        assert_eq!((shown.len(), more), (10, 0));
        let states = vec!["idle"; 12];
        let list = threads(&states);
        let (shown, more) = visible(&list, Some("t11"), false);
        assert_eq!((shown.len(), more), (9, 3), "the shown one is kept besides the eight");
        assert_eq!(visible(&list, None, true).0.len(), 12);
    }
}
