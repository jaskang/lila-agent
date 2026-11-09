//! lila-app: watch and steer a Lila team. Talk to the PM in its terminal,
//! answer what needs you, follow the task board.

mod app;
mod compose;
mod conversation;
mod data;
mod detail;
mod dialog;
mod prefs;
mod requests;
mod settings;
mod tree;
mod style;
mod ui;
mod usage;
mod wizard;

use gpui::*;

fn main() {
    // lilad and the members it starts inherit this PATH: give them the shell's,
    // before any thread runs.
    // SAFETY: nothing else runs yet.
    unsafe { std::env::set_var("PATH", usage::login_path()) };
    // The component icons (Lucide) come from gpui-kit's bundled assets, Lila's own from assets/lila.
    gpui::application().with_assets(style::Assets).run(|cx: &mut App| {
        gpui::init(cx);
        lila_term::view::init(cx);
        // Terminals sit on the glass cards, so they leave their background unpainted.
        lila_term::theme::set_transparent(true);
        style::install(cx);
        let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // No separate title bar: the traffic lights sit in the sidebar card's first
            // row, centered on it (window padding 8 + 14 down, 8 + 16 in), and the
            // first row of the sidebar and center cards drags the window (see ui::drag_bar).
            titlebar: Some(TitlebarOptions { title: Some("Lila".into()), appears_transparent: true, traffic_light_position: Some(point(px(24.), px(22.))) }),
            // Graphite frosted glass: the root paints a translucent graphite over the blur.
            window_background: WindowBackgroundAppearance::Blurred,
            ..Default::default()
        };
        let (_window, _view) = gpui::open_window(options, cx, |window, cx| {
            let view = cx.new(|cx| app::LilaApp::new(window, cx));
            #[cfg(feature = "shots")]
            {
                shots::start(window, cx);
                // Checks without a mouse: open this task's panel on start.
                if let Ok(task) = std::env::var("LILA_APP_OPEN_TASK") {
                    let view = view.clone();
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                            let _ = cx.update(|_, cx| view.update(cx, |v, cx| v.show_task(task, cx)));
                        })
                        .detach();
                }
                // As if back after being away since this many minutes ago (`LILA_APP_AWAY=360`).
                if let Ok(m) = std::env::var("LILA_APP_AWAY").map(|m| m.parse::<i64>().unwrap_or(60)) {
                    view.update(cx, |v, _| v.recap = Some(data::now_ms() - m * 60_000));
                }
                // Select this request (`LILA_APP_REQUEST=T3`) or show a member's terminal (`LILA_APP_MEMBER=pm`).
                for (var, member) in [("LILA_APP_REQUEST", false), ("LILA_APP_MEMBER", true)] {
                    if let Ok(id) = std::env::var(var) {
                        let view = view.clone();
                        window
                            .spawn(cx, async move |cx| {
                                cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                                let _ = cx.update(|_, cx| {
                                    view.update(cx, |v, cx| {
                                        if member {
                                            v.member = Some(id);
                                            cx.notify();
                                        } else {
                                            v.select_request(id, cx);
                                        }
                                    })
                                });
                            })
                            .detach();
                    }
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
                // Open the shown project's settings once it has loaded.
                if let Ok(page) = std::env::var("LILA_APP_SETTINGS") {
                    let view = view.clone();
                    let page = crate::settings::Page::parse(&page);
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                            let _ = cx.update(|window, cx| view.update(cx, |v, cx| {
                                v.open_settings_at(page, window, cx);
                                if let Ok(role) = std::env::var("LILA_APP_SETTINGS_ADD") {
                                    v.add_member(&role, window, cx);
                                }
                            }));
                        })
                        .detach();
                }
                // Press keys as a person would (`cmd-,`, space-separated), through the
                // window's own dispatch: checks that shortcuts reach the app.
                if let Ok(keys) = std::env::var("LILA_APP_KEYS") {
                    let keys: &'static str = Box::leak(keys.into_boxed_str());
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                            let strokes: Vec<gpui::Keystroke> = keys.split_whitespace().filter_map(|k| gpui::Keystroke::parse(k).ok()).collect();
                            for stroke in strokes {
                                {
                                    let _ = cx.update(|window, cx| window.dispatch_keystroke(stroke, cx));
                                }
                            }
                        })
                        .detach();
                }
                // Add an account (`claude:<name>`): the usage page, its login started.
                if let Ok(spec) = std::env::var("LILA_APP_SIGNIN") {
                    let view = view.clone();
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                            let _ = cx.update(|window, cx| view.update(cx, |v, cx| v.dev_signin(&spec, window, cx)));
                        })
                        .detach();
                }
                // Ask about removing this project from the list.
                if let Ok(id) = std::env::var("LILA_APP_REMOVE") {
                    let view = view.clone();
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                            let _ = cx.update(|_, cx| view.update(cx, |v, _| v.removing = v.snap.projects.iter().find(|p| p.id == id).cloned()));
                        })
                        .detach();
                }
                // The role library on this role, its final prompt shown.
                if let Ok(role) = std::env::var("LILA_APP_PROMPT") {
                    let view = view.clone();
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                            let _ = cx.update(|window, cx| view.update(cx, |v, cx| v.dev_preview(&role, window, cx)));
                        })
                        .detach();
                }
                // Open the global settings: `roles`, `usage` or `about`.
                if let Ok(page) = std::env::var("LILA_APP_PREFS") {
                    let view = view.clone();
                    let page = match page.as_str() {
                        "about" => crate::prefs::PrefPage::About,
                        "usage" => crate::prefs::PrefPage::Usage,
                        _ => crate::prefs::PrefPage::Roles,
                    };
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                            let _ = cx.update(|window, cx| view.update(cx, |v, cx| v.open_prefs(page, window, cx)));
                        })
                        .detach();
                }
                // The new-project dialog, on this folder (empty: none yet).
                if let Ok(dir) = std::env::var("LILA_APP_NEW") {
                    let view = view.clone();
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                            let _ = cx.update(|window, cx| view.update(cx, |v, cx| {
                                v.new_project(window, cx);
                                if !dir.is_empty() {
                                    v.wizard_set_folder(dir.into(), window, cx);
                                }
                            }));
                        })
                        .detach();
                }
                // The new-task form, once the shown project has loaded.
                if let Ok(to) = std::env::var("LILA_APP_COMPOSE") {
                    let view = view.clone();
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                            let _ = cx.update(|window, cx| view.update(cx, |v, cx| v.compose(Some(to).filter(|t| !t.is_empty()), None, window, cx)));
                        })
                        .detach();
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
        // A window behind others isn't drawn again: keep it in front while shooting.
        cx.activate(true);
        window.activate_window();
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
        // The window is translucent glass; lay it on a dark desktop color so
        // the PNG looks like it does over a wallpaper (without the blur).
        let mut pixels = image.as_raw().clone();
        for px in pixels.as_chunks_mut::<4>().0 {
            let a = px[3] as f32 / 255.;
            for (c, bg) in px[..3].iter_mut().zip([0x1c_u8, 0x1d, 0x2e]) {
                *c = (*c as f32 * a + bg as f32 * (1. - a)).round() as u8;
            }
            px[3] = 255;
        }
        if let Ok(mut w) = encoder.write_header() {
            let _ = w.write_image_data(&pixels);
        }
    }
}
