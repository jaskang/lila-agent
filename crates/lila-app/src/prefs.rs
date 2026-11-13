//! Global settings, a dialog: about, and where Lila keeps things. The project's own settings are
//! its defaults in `.lila/project.toml`.

use crate::app::LilaApp;
use crate::data;
use crate::dialog::{self, Shell, group, row, value_text};
use gpui::component::{ActiveTheme, Icon, IconName, Sizable, h_flex};
use gpui::prelude::*;
use gpui::*;

impl LilaApp {
    pub fn open_prefs(&mut self, cx: &mut Context<Self>) {
        self.dialog_offset = Point::default();
        self.prefs = true;
        cx.notify();
    }

    fn about_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let paths = self.paths().clone();
        let tilde = |p: std::path::PathBuf| data::tail(&data::tilde(&p.to_string_lossy()), 44);
        let daemon = data::daemon_line(&self.snap);
        dialog::scroll(
            "prefs-about",
            vec![
                group(None, vec![row("版本", None, value_text(env!("CARGO_PKG_VERSION"), cx), cx), row("lilad", None, value_text(daemon, cx), cx)], cx),
                group(None, vec![row("数据库", None, value_text(tilde(paths.db()), cx), cx), row("日志", None, value_text(tilde(paths.state.clone()), cx), cx)], cx),
            ],
        )
    }

    pub fn prefs_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.prefs {
            return None;
        }
        let theme = cx.theme().clone();
        let body = self.about_page(cx);
        let note = self.notice.clone().map(|e| h_flex().gap_2().text_color(theme.red).child(Icon::new(IconName::CircleAlert).small()).child(div().truncate().child(e)).into_any_element());
        Some(self.dialog(
            Shell {
                id: "prefs",
                nav: Vec::new(),
                title: "关于".into(),
                body,
                note,
                saving: false,
                save_label: "保存",
                can_save: false,
                on_save: Box::new(|_, _| {}),
                on_close: Box::new(|this, _| this.prefs = false),
                footer: false,
                compact: false,
            },
            cx,
        ))
    }
}
