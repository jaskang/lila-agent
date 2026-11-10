//! Global settings, a dialog: usage per account (the accounts threads run on), and where Lila
//! keeps things. The project's own settings are its defaults in `.lila/project.toml`.

use crate::app::LilaApp;
use crate::data;
use crate::dialog::{self, Act, Nav, Shell, group, row, value_text};
use gpui::component::{ActiveTheme, Icon, IconName, Sizable, h_flex};
use gpui::prelude::*;
use gpui::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefPage {
    Usage,
    About,
}

pub struct Prefs {
    pub page: PrefPage,
}

impl LilaApp {
    pub fn open_prefs(&mut self, page: PrefPage, cx: &mut Context<Self>) {
        self.dialog_offset = Point::default();
        self.prefs = Some(Prefs { page });
        cx.notify();
    }

    fn about_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let paths = self.paths().clone();
        let tilde = |p: std::path::PathBuf| data::tail(&data::tilde(&p.to_string_lossy()), 44);
        let daemon = if self.snap.daemon { "运行中" } else { "没有运行" };
        dialog::scroll(
            "prefs-about",
            vec![
                group(None, vec![row("版本", None, value_text(env!("CARGO_PKG_VERSION"), cx), cx), row("lilad", None, value_text(daemon, cx), cx), row("libghostty-vt", None, value_text(lila_term::ghostty_version(), cx), cx)], cx),
                group(None, vec![row("设置", None, value_text(tilde(paths.config.clone()), cx), cx), row("数据库", None, value_text(tilde(paths.db()), cx), cx), row("日志", None, value_text(tilde(paths.state.clone()), cx), cx)], cx),
            ],
        )
    }

    pub fn prefs_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.prefs.as_ref()?;
        let theme = cx.theme().clone();
        let page = p.page;
        let (title, body) = match page {
            PrefPage::Usage => ("用量", self.usage_page(cx)),
            PrefPage::About => ("关于", self.about_page(cx)),
        };
        let note = self.notice.clone().map(|e| h_flex().gap_2().text_color(theme.red).child(Icon::new(IconName::CircleAlert).small()).child(div().truncate().child(e)).into_any_element());
        let go = |pg: PrefPage| -> Act {
            Box::new(move |this: &mut LilaApp, _| {
                if let Some(p) = this.prefs.as_mut() {
                    p.page = pg;
                }
            })
        };
        let nav = vec![
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
                saving: false,
                save_label: "保存",
                can_save: false,
                on_save: Box::new(|_, _| {}),
                on_close: Box::new(|this, _| this.prefs = None),
                footer: false,
                compact: false,
            },
            cx,
        ))
    }
}
