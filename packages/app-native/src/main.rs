//! Desktop entry point for Lila (`gpui-kit` + `packages/core`).

use gpui_kit::component::TitleBar;
use gpui_kit::{
    AppContext as _, Bounds, WindowBackgroundAppearance, WindowBounds, WindowOptions, px, size,
};

use lila_native::store::AppStore;
use lila_native::ui::app::{LilaApp, bind_global_keys};
use lila_native::ui::theme_setup;

fn main() {
    env_logger::init();

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            theme_setup::init_themes(cx);
            bind_global_keys(cx);
            AppStore::init(cx);

            let bounds = Bounds::centered(None, size(px(1240.0), px(820.0)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(760.0), px(520.0))),
                window_background: WindowBackgroundAppearance::Blurred,
                ..TitleBar::window_options()
            };

            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| LilaApp::new(window, cx))
            })
            .expect("failed to open Lila main window");

            cx.activate(true);
        });
}
