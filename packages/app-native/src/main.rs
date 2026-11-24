//! Desktop entry point for Lila (`gpui-kit` + `packages/core`).

use gpui_kit::{
    AppContext as _, Bounds, Styled as _, TitlebarOptions, WindowBackgroundAppearance,
    WindowBounds, WindowOptions, base::Root, point, px, size, transparent_black,
};

use lila_native::store::AppStore;
use lila_native::ui::app::{LilaApp, bind_global_keys};
use lila_native::ui::theme_setup;

fn main() {
    env_logger::init();
    let demo_mode = std::env::args().any(|arg| arg == "--demo");
    let initial_theme_pref = AppStore::persisted_theme_preference();

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            theme_setup::init_themes(initial_theme_pref, cx);
            bind_global_keys(cx);
            AppStore::init_with_demo(demo_mode, cx);

            let bounds = Bounds::centered(None, size(px(1240.0), px(820.0)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(760.0), px(520.0))),
                window_background: WindowBackgroundAppearance::Blurred,
                titlebar: Some(TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(15.0), px(16.0))),
                }),
                app_owns_titlebar_drag: true,
                ..Default::default()
            };

            cx.open_window(options, |window, cx| {
                let app = cx.new(|cx| LilaApp::new(window, cx));
                // GPUI Kit's default Root fill would cover the native window material.
                cx.new(|cx| Root::new(app, window, cx).bg(transparent_black()))
            })
            .expect("failed to open Lila main window");

            cx.activate(true);
        });
}
