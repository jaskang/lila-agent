//! lila-app: watch and steer a Lila team. Talk to the PM in its terminal,
//! answer what needs you, follow the task board.

mod app;
mod compose;
mod data;
mod detail;
mod ui;

use gpui::*;

fn main() {
    // The component icons (Lucide) come from gpui-kit's bundled assets.
    gpui::application().with_assets(gpui::assets::Assets).run(|cx: &mut App| {
        gpui::init(cx);
        lila_term::view::init(cx);
        gpui::component::Theme::change(gpui::component::ThemeMode::Dark, None, cx);
        let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // No separate title bar: the traffic lights sit in the sidebar's top row
            // and the top row of every column drags the window (see ui::drag_bar).
            titlebar: Some(TitlebarOptions { title: Some("Lila".into()), appears_transparent: true, traffic_light_position: Some(point(px(12.), px(14.))) }),
            ..Default::default()
        };
        let (_window, _view) = gpui::open_window(options, cx, |window, cx| {
            let view = cx.new(|cx| app::LilaApp::new(window, cx));
            #[cfg(feature = "shots")]
            {
                shots::start(window, cx);
                // Checks without a mouse: open this task's panel on start.
                if let Ok(task) = std::env::var("LILA_APP_OPEN_TASK") {
                    view.update(cx, |v, _| v.task = Some(task));
                }
                // Switch through these projects (comma-separated), one every 3 s.
                if let Ok(list) = std::env::var("LILA_APP_SWITCH") {
                    let view = view.clone();
                    window
                        .spawn(cx, async move |cx| {
                            for id in list.split(',').map(str::to_owned) {
                                cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
                                eprintln!("[lila-app] {} switch to {id}", data::clock_ms());
                                view.update(cx, |v, cx| v.select_project(id, cx));
                            }
                        })
                        .detach();
                }
                if let Ok(to) = std::env::var("LILA_APP_COMPOSE") {
                    view.update(cx, |v, cx| v.compose(Some(to).filter(|t| !t.is_empty()), None, window, cx));
                }
            }
            view
        })
        .expect("open the Lila window");
        cx.activate(true);
    });
}

/// `--features shots`: the window saves itself as a PNG every 2 s to `$LILA_APP_SHOT`,
/// so a change can be checked without screen-recording permission.
#[cfg(feature = "shots")]
mod shots {
    use gpui::*;

    pub fn start(window: &mut Window, cx: &mut App) {
        let Ok(path) = std::env::var("LILA_APP_SHOT") else { return };
        window
            .spawn(cx, async move |cx| {
                loop {
                    cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
                    let path = path.clone();
                    if cx.update(|window, _| save(window, &path)).is_err() {
                        break;
                    }
                }
            })
            .detach();
    }

    /// The close button's frame, as AppKit lays it out (the PNG has no window buttons).
    #[cfg(target_os = "macos")]
    fn log_buttons() {
        use objc2_app_kit::{NSApplication, NSWindowButton};
        let Some(mtm) = objc2::MainThreadMarker::new() else { return };
        for w in NSApplication::sharedApplication(mtm).windows().iter() {
            if let Some(b) = w.standardWindowButton(NSWindowButton::CloseButton) {
                let f = b.frame();
                let m = w.standardWindowButton(NSWindowButton::MiniaturizeButton).map(|m| m.frame().origin.x).unwrap_or_default();
                eprintln!("[lila-app] close button {}x{} at ({}, {}), minimize at x {m}", f.size.width, f.size.height, f.origin.x, f.origin.y);
            }
        }
    }

    fn save(window: &Window, path: &str) {
        #[cfg(target_os = "macos")]
        log_buttons();
        let Ok(image) = window.render_to_image() else { return };
        let Ok(file) = std::fs::File::create(path) else { return };
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), image.width(), image.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        if let Ok(mut w) = encoder.write_header() {
            let _ = w.write_image_data(image.as_raw());
        }
    }
}
