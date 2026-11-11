//! Going back to a message (A1, docs/architecture-v4.md §11): the person takes one of their own messages and
//! the thread's files go back to how they were just before it was sent; the conversation stays. The window
//! only asks and draws: lilad is asked what that would change (`thread.restore` with `dry_run`), the answer
//! is shown, and only on a yes are the files put back.

use crate::app::LilaApp;
use crate::dialog;
use crate::panel::mark;
use crate::style::{self, muted, muted2};
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::scroll::ScrollableElement;
use gpui::component::{Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui::prelude::*;
use gpui::*;
use serde_json::{Value, json};
use std::collections::HashSet;

/// The first protocol that has checkpoints; an older lilad has no `thread.restore`, and draws `restored` marks as speech.
pub const PROTOCOL: u32 = 2;

/// What the window is doing about going back.
#[derive(Default)]
pub struct Restore {
    /// The dry run under way, as (thread, checkpoint): its button turns.
    pub checking: Option<(String, i64)>,
    /// What the dry run said, put to the person.
    pub ask: Option<Ask>,
    /// The threads whose files this window is putting back now (lilad's `restoring` says so too, once it is read).
    pub going: HashSet<String>,
}

pub struct Ask {
    thread: String,
    checkpoint: i64,
    plan: Plan,
}

/// What a restore would do to the folder, as the dry run says it (`thread.restore`'s reply).
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    /// Files that differ and are put back.
    pub changed: Vec<String>,
    /// Files that are gone and are made again.
    pub recreated: Vec<String>,
    /// Files that were not there then and are deleted.
    pub deleted: Vec<String>,
    /// How many of each there are in all (the lists are cut when long).
    pub counts: [usize; 3],
    /// The lists are not all of them.
    pub truncated: bool,
    /// Commits were made since; they stay.
    pub head_moved: bool,
}

impl Plan {
    pub fn of(v: &Value) -> Plan {
        let list = |k: &str| v[k].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(String::from)).collect::<Vec<_>>();
        let (changed, recreated, deleted) = (list("changed"), list("recreated"), list("deleted"));
        let count = |k: &str, listed: &[String]| v["counts"][k].as_u64().map_or(listed.len(), |n| n as usize);
        let counts = [count("changed", &changed), count("recreated", &recreated), count("deleted", &deleted)];
        Plan { changed, recreated, deleted, counts, truncated: v["truncated"] == true, head_moved: v["head_moved"] == true }
    }

    /// Nothing would change.
    pub fn is_empty(&self) -> bool {
        self.counts.iter().all(|n| *n == 0)
    }

    /// The numbers in a line: `放回 3 个文件，重新做出 1 个，删除 2 个`; what is zero is left out.
    pub fn summary(&self) -> String {
        let parts: Vec<String> = ["放回", "重新做出", "删除"].iter().zip(self.counts).filter(|(_, n)| *n > 0).map(|(verb, n)| format!("{verb} {n} 个")).collect();
        match parts.split_first() {
            Some((first, rest)) => std::iter::once(format!("{first}文件")).chain(rest.iter().cloned()).collect::<Vec<_>>().join("，"),
            None => String::new(),
        }
    }

    /// The files in order, each with what becomes of it (`panel::mark`'s statuses).
    pub fn files(&self) -> Vec<(&'static str, &str)> {
        self.changed.iter().map(|p| ("modified", p.as_str())).chain(self.recreated.iter().map(|p| ("added", p.as_str()))).chain(self.deleted.iter().map(|p| ("deleted", p.as_str()))).collect()
    }

    /// How many files there are that the lists do not name.
    pub fn unlisted(&self) -> usize {
        self.counts.iter().sum::<usize>().saturating_sub(self.changed.len() + self.recreated.len() + self.deleted.len())
    }
}

/// Is the thread such that its files may be put back now, as far as the window can tell? Its engine is at rest, nothing
/// else is being done to it (`going`: this window is doing a restore), it is not merged, and lilad speaks protocol 2.
/// (lilad's own `restorable` has the last word on the rest: the folder shared with a thread at work, a message waiting.)
pub fn can_go_back(t: &Value, protocol: Option<u32>, going: bool) -> bool {
    protocol.is_some_and(|p| p >= PROTOCOL) && !going && matches!(t["state"].as_str(), Some("idle" | "ended")) && !t["merged_ms"].is_number() && t["merging"] != true && t["restoring"] != true
}

/// Does the thread turn: its engine works, is starting, or its files are being put back?
pub fn turning(t: &Value, going: bool) -> bool {
    going || t["restoring"] == true || matches!(t["state"].as_str(), Some("running" | "starting"))
}

/// The checkpoint to go back to for a message of the person's: the one taken before it, if lilad takes it back now.
pub fn checkpoint_of(checkpoints: &[Value], item: &str) -> Option<i64> {
    checkpoints.iter().find(|c| c["kind"] == "turn" && c["item"] == item && c["restorable"] == true).and_then(|c| c["id"].as_i64())
}

/// The checkpoint a `restored` mark undoes by (the state before the restore), if it is still there to go back to.
pub fn undo_of(checkpoints: &[Value], undo: Option<i64>) -> Option<i64> {
    let undo = undo?;
    checkpoints.iter().find(|c| c["id"].as_i64() == Some(undo) && c["restorable"] == true).map(|_| undo)
}

impl LilaApp {
    /// The protocol the running lilad speaks, if it said.
    fn daemon_protocol(&self) -> Option<u32> {
        self.snap.daemon_info.as_ref().and_then(|i| i.protocol)
    }

    /// The shown thread may have its files put back now.
    pub fn can_go_back_now(&self) -> bool {
        let Some(t) = self.shown_thread() else { return false };
        can_go_back(&t, self.daemon_protocol(), self.restore.going.contains(&crate::ui::s(&t, "id")))
    }

    /// The shown thread's files are being put back (by this window or, as lilad says, by anyone).
    pub fn restoring(&self, thread: &str) -> bool {
        self.restore.going.contains(thread) || self.snap.threads.iter().any(|t| t["id"] == thread && t["restoring"] == true)
    }

    /// The checkpoint a message of the person's can go back to, while the thread allows it.
    pub fn restore_target(&self, item: &str) -> Option<i64> {
        if !self.can_go_back_now() {
            return None;
        }
        checkpoint_of(&self.conv.as_ref()?.checkpoints, item)
    }

    /// The checkpoint a `restored` mark undoes by, while the thread allows it.
    pub fn undo_target(&self, undo: Option<i64>) -> Option<i64> {
        if !self.can_go_back_now() {
            return None;
        }
        undo_of(&self.conv.as_ref()?.checkpoints, undo)
    }

    /// The first step: lilad is asked what going back to `checkpoint` would change, without doing it; the answer is put to the person.
    pub fn ask_restore(&mut self, thread: String, checkpoint: i64, window: &mut Window, cx: &mut Context<Self>) {
        if self.restore.checking.is_some() || self.restoring(&thread) {
            return;
        }
        self.restore.checking = Some((thread.clone(), checkpoint));
        self.notice = None;
        cx.notify();
        self.request_result("thread.restore", json!({"thread": thread, "checkpoint": checkpoint, "dry_run": true}), window, cx, move |this, r, _, cx| {
            this.restore.checking = None;
            match r {
                // The person went to another thread meanwhile: the answer is of no use there.
                Ok(_) if this.thread.as_deref() != Some(thread.as_str()) => {}
                Ok(v) => this.restore.ask = Some(Ask { thread, checkpoint, plan: Plan::of(&v) }),
                Err(e) => this.notice = Some(format!("{e:#}")),
            }
            cx.notify();
        });
    }

    /// The second step, after the person said yes: the files are put back, off the UI thread (it takes a few seconds on a big folder).
    fn go_back(&mut self, thread: String, checkpoint: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.restore.ask = None;
        self.restore.going.insert(thread.clone());
        self.notice = None;
        cx.notify();
        self.request_result("thread.restore", json!({"thread": thread, "checkpoint": checkpoint}), window, cx, move |this, r, _, cx| {
            this.restore.going.remove(&thread);
            if let Err(e) = r {
                this.notice = Some(format!("{e:#}"));
            }
            // Success or not, the folder may have changed (a failure can be half done): the conversation and the panel read again.
            this.refresh_now();
            if this.thread.as_deref() == Some(thread.as_str()) {
                this.panel_refresh(cx);
            }
            cx.notify();
        });
    }

    /// The quiet button on a message of the person's: it turns while lilad is asked what going back would do.
    pub fn restore_button(&self, id: &str, thread: &str, checkpoint: i64, cx: &mut Context<Self>) -> Button {
        let quiet = style::quiet(cx);
        let (thread, checking) = (thread.to_owned(), self.restore.checking == Some((thread.to_owned(), checkpoint)));
        Button::new(SharedString::from(format!("restore-{id}")))
            .custom(quiet)
            .xsmall()
            .icon(Icon::new(IconName::Undo2).text_color(muted()))
            .loading(checking)
            .tooltip("回到这里")
            .on_click(cx.listener(move |this, _, window, cx| this.ask_restore(thread.clone(), checkpoint, window, cx)))
    }

    /// Where the person went back: a quiet mark in the conversation, with 「撤销」 when the state before is still there to go back to.
    pub fn restored_row(&self, id: &str, undo: Option<i64>, cx: &mut Context<Self>) -> AnyElement {
        let thread = self.conv.as_ref().map(|c| c.thread.clone()).unwrap_or_default();
        let undo = self.undo_target(undo);
        let checking = undo.is_some_and(|n| self.restore.checking == Some((thread.clone(), n)));
        h_flex()
            .w_full()
            .h(px(24.))
            .gap_2()
            .px_1()
            .child(Icon::new(IconName::Undo2).size(px(11.)).text_color(muted2()))
            .child(div().text_xs().text_color(muted2()).child("已回到这里"))
            .child(div().flex_1().h(px(1.)).bg(style::divider()))
            .children(undo.map(|n| {
                h_flex()
                    .id(SharedString::from(format!("undo-{id}")))
                    .h(px(22.))
                    .px_1()
                    .gap_1()
                    .items_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(muted2())
                    .hover(|d| d.bg(style::hover()).text_color(muted()))
                    .child("撤销")
                    .when(checking, |d| d.child(gpui::component::spinner::Spinner::new().xsmall().color(muted2())))
                    .on_click(cx.listener(move |this, _, window, cx| this.ask_restore(thread.clone(), n, window, cx)))
            }))
            .into_any_element()
    }

    /// The question before the files go back: how many of which kind, and which; or, when nothing would change, only that.
    pub fn restore_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let quiet = style::quiet(cx);
        let ask = self.restore.ask.as_ref()?;
        let (thread, checkpoint, plan) = (ask.thread.clone(), ask.checkpoint, &ask.plan);
        let close: dialog::Act = Box::new(|this, _| this.restore.ask = None);
        let no = |id: &'static str, label: &'static str, cx: &mut Context<Self>| {
            Button::new(id)
                .custom(quiet)
                .label(label)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.restore.ask = None;
                    cx.notify();
                }))
                .into_any_element()
        };
        if plan.is_empty() {
            return Some(dialog::small("restore-dialog", "回到这里".into(), Some("文件已经是这个样子。".into()), None, vec![no("restore-no", "关闭", cx)], close, cx));
        }
        let unlisted = plan.unlisted();
        let list = v_flex()
            .id("restore-files")
            .max_h(px(200.))
            .overflow_y_scrollbar()
            .px_3()
            .py_2()
            .gap(px(2.))
            .rounded(px(6.))
            .bg(style::well())
            .font_family(lila_term::theme::FONT)
            .text_size(px(12.))
            .children(plan.files().into_iter().map(|(status, path)| {
                let (letter, color) = mark(status);
                h_flex()
                    .gap_2()
                    .child(div().flex_none().w(px(10.)).text_size(px(11.)).font_semibold().text_color(color).child(letter))
                    .child(div().flex_1().min_w_0().truncate().text_color(muted()).child(path.to_owned()))
            }))
            .when(unlisted > 0, |d| d.child(div().text_color(muted2()).child(format!("…还有 {unlisted} 个"))));
        let content = v_flex().gap_2().child(list).when(plan.head_moved, |d| d.child(div().text_xs().text_color(muted2()).child("之后的提交保留，它们的内容会显示为未提交的改动。")));
        Some(dialog::small(
            "restore-dialog",
            "回到这里？".into(),
            Some(format!("{}。", plan.summary())),
            Some(content.into_any_element()),
            vec![
                no("restore-no", "取消", cx),
                Button::new("restore-yes").outline().text_color(style::red()).label("回到这里").on_click(cx.listener(move |this, _, window, cx| this.go_back(thread.clone(), checkpoint, window, cx))).into_any_element(),
            ],
            close,
            cx,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{Plan, can_go_back, checkpoint_of, turning, undo_of};
    use serde_json::json;

    fn thread(state: &str) -> serde_json::Value {
        json!({"id": "t1", "state": state, "merged_ms": null, "merging": false, "restoring": false})
    }

    #[test]
    fn the_action_is_offered_only_while_the_thread_is_at_rest_and_lilad_knows_checkpoints() {
        for state in ["idle", "ended"] {
            assert!(can_go_back(&thread(state), Some(2), false), "{state}");
        }
        for state in ["running", "waiting", "starting"] {
            assert!(!can_go_back(&thread(state), Some(2), false), "{state}");
        }
        let with = |k: &str, v: serde_json::Value| {
            let mut t = thread("idle");
            t[k] = v;
            t
        };
        assert!(!can_go_back(&with("merged_ms", json!(1_700_000_000_000_i64)), Some(2), false), "merged");
        assert!(!can_go_back(&with("merging", json!(true)), Some(2), false), "being merged");
        assert!(!can_go_back(&with("restoring", json!(true)), Some(2), false), "being restored");
        assert!(!can_go_back(&thread("idle"), Some(2), true), "this window restores it");
        assert!(!can_go_back(&thread("idle"), Some(1), false), "an older lilad");
        assert!(!can_go_back(&thread("idle"), None, false), "a lilad that does not say");
        assert!(can_go_back(&thread("idle"), Some(3), false), "a newer one");
    }

    #[test]
    fn a_thread_turns_while_it_works_or_its_files_are_put_back() {
        assert!(turning(&thread("running"), false));
        assert!(turning(&thread("starting"), false));
        assert!(!turning(&thread("idle"), false));
        assert!(turning(&thread("idle"), true));
        let mut t = thread("idle");
        t["restoring"] = json!(true);
        assert!(turning(&t, false));
    }

    #[test]
    fn a_message_gets_the_checkpoint_taken_before_it_when_that_can_be_restored() {
        let rows = json!([
            {"id": 7, "kind": "restore", "item": "", "restorable": true},
            {"id": 5, "kind": "turn", "item": "u2", "restorable": true},
            {"id": 4, "kind": "turn", "item": null, "restorable": true},
            {"id": 3, "kind": "turn", "item": "u1", "restorable": false},
            {"id": 2, "kind": "turn", "item": "u0", "restorable": true},
        ]);
        let rows = rows.as_array().unwrap();
        assert_eq!(checkpoint_of(rows, "u2"), Some(5));
        assert_eq!(checkpoint_of(rows, "u0"), Some(2));
        assert_eq!(checkpoint_of(rows, "u1"), None, "lilad would not take it back now");
        assert_eq!(checkpoint_of(rows, "u9"), None, "no checkpoint for it");
        assert_eq!(checkpoint_of(rows, ""), None, "a restore's own checkpoint belongs to no message");
        assert_eq!(checkpoint_of(&[], "u2"), None);
    }

    #[test]
    fn a_mark_can_be_undone_while_the_state_before_is_there_to_go_back_to() {
        let rows = json!([{"id": 7, "kind": "restore", "item": "", "restorable": true}, {"id": 6, "kind": "restore", "item": "", "restorable": false}]);
        let rows = rows.as_array().unwrap();
        assert_eq!(undo_of(rows, Some(7)), Some(7));
        assert_eq!(undo_of(rows, Some(6)), None, "not now");
        assert_eq!(undo_of(rows, Some(2)), None, "long gone");
        assert_eq!(undo_of(rows, None), None, "the mark names none");
    }

    #[test]
    fn a_plan_says_how_many_of_each_in_one_line_and_what_is_cut() {
        let p = Plan::of(&json!({"changed": ["a.rs", "b.rs"], "recreated": ["c.txt"], "deleted": ["d"], "counts": {"changed": 3, "recreated": 1, "deleted": 1}, "truncated": true, "head_moved": true}));
        assert_eq!(p.summary(), "放回 3 个文件，重新做出 1 个，删除 1 个");
        assert_eq!((p.unlisted(), p.truncated, p.head_moved, p.is_empty()), (1, true, true, false));
        assert_eq!(p.files(), [("modified", "a.rs"), ("modified", "b.rs"), ("added", "c.txt"), ("deleted", "d")]);
        let only = Plan::of(&json!({"changed": [], "recreated": [], "deleted": ["x", "y"], "counts": {"changed": 0, "recreated": 0, "deleted": 2}}));
        assert_eq!(only.summary(), "删除 2 个文件");
        assert!(!only.truncated && !only.head_moved);
        // Without counts the lists are the numbers.
        assert_eq!(Plan::of(&json!({"changed": ["a"]})).counts, [1, 0, 0]);
        let none = Plan::of(&json!({"changed": [], "recreated": [], "deleted": [], "counts": {"changed": 0, "recreated": 0, "deleted": 0}}));
        assert!(none.is_empty());
        assert_eq!(none.summary(), "");
        assert!(Plan::of(&json!(null)).is_empty());
    }
}
