//! Keys the window binds for itself must still reach a terminal as typing.

use gpui::{AppContext, Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, Render, Styled, TestAppContext, VisualTestContext, Window, div};

struct Probe {
    focus: FocusHandle,
    keys: Vec<String>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context(lila_term::view::KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, _| {
                let m = ev.keystroke.modifiers;
                let prefix = if m.platform { "cmd-" } else if m.shift { "shift-" } else { "" };
                this.keys.push(format!("{prefix}{}", ev.keystroke.key));
            }))
            .size_full()
    }
}

#[gpui::test]
fn tab_shift_tab_and_cmd_c_reach_the_terminal(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui::init(cx);
        lila_term::view::init(cx);
    });
    let mut probe = None;
    let window = cx.add_window(|window, cx| {
        let p = cx.new(|cx| Probe { focus: cx.focus_handle(), keys: Vec::new() });
        probe = Some(p.clone());
        gpui::base::Root::new(p, window, cx)
    });
    let probe = probe.unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.update(|window, cx| {
        let focus = probe.read(cx).focus.clone();
        window.focus(&focus, cx);
    });
    cx.simulate_keystrokes("tab shift-tab cmd-c a");
    let keys = cx.update(|_, cx| probe.read(cx).keys.clone());
    assert_eq!(keys, ["tab", "shift-tab", "cmd-c", "a"]);
}
