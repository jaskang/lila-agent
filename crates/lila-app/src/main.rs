//! lila-app: Lila's window. Projects and their threads on the left, the shown thread's
//! conversation (or Claude Code's terminal) in the middle.

mod app;
mod composer;
mod conversation;
mod data;
mod dialog;
mod engines;
mod panel;
mod prefs;
mod restore;
mod settings;
mod sidebar;
mod style;
mod ui;
mod usage;

use gpui::*;

fn main() {
    // lilad and the engines it starts inherit this PATH: give them the shell's,
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
        // `--features shots`: `LILA_APP_SIZE=1000x700` opens the window that size, to look at narrow ones.
        #[allow(unused_mut)]
        let mut window_size = size(px(1440.), px(900.));
        #[cfg(feature = "shots")]
        if let Some((w, h)) = std::env::var("LILA_APP_SIZE").ok().as_deref().and_then(|s| s.split_once('x')).and_then(|(w, h)| Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))) {
            window_size = size(px(w), px(h));
        }
        let bounds = Bounds::centered(None, window_size, cx);
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
                // Show this thread (`LILA_APP_THREAD=t1234abcd`).
                if let Ok(id) = std::env::var("LILA_APP_THREAD") {
                    let view = view.clone();
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                            let _ = cx.update(|_, cx| view.update(cx, |v, cx| v.show_thread(id, cx)));
                        })
                        .detach();
                }
                // Put the conversation in a state to look at (`top`, `row:N`, `open:N`, `openall:N`, `focus`, `key:<keystroke>`, `fill:<words>`, `attach:<path>`, `menu`, `show:<id>`; see `dev_conv`, `dev_composer`).
                if let Ok(spec) = std::env::var("LILA_APP_CONV") {
                    let view = view.clone();
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(3500)).await;
                            let _ = cx.update(|_, cx| view.update(cx, |v, cx| v.dev_conv(&spec, cx)));
                            for step in spec.split(',') {
                                let _ = cx.update(|window, cx| {
                                    if step.starts_with("fill:") || step.starts_with("attach:") || step.starts_with("show:") || step == "menu" {
                                        view.update(cx, |v, cx| v.dev_composer(step, window, cx));
                                    } else if step == "focus" {
                                        if let Some(h) = view.read(cx).conv_focus(cx) {
                                            window.focus(&h, cx);
                                        }
                                    } else if let Some(key) = step.strip_prefix("key:")
                                        && let Ok(stroke) = gpui::Keystroke::parse(Box::leak(key.to_owned().into_boxed_str()))
                                    {
                                        window.dispatch_keystroke(stroke, cx);
                                    }
                                });
                                cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                            }
                        })
                        .detach();
                }
                // A new session's box in a project, with choices made: `LILA_APP_NEW=shop,engine=codex,effort=low,permission=confirm`.
                if let Ok(spec) = std::env::var("LILA_APP_NEW") {
                    let view = view.clone();
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(2000)).await;
                            let _ = cx.update(|window, cx| view.update(cx, |v, cx| v.dev_new(&spec, window, cx)));
                        })
                        .detach();
                }
                // Add a folder as a project, as the folder picker would (`LILA_APP_ADD=<dir>`).
                if let Ok(dir) = std::env::var("LILA_APP_ADD") {
                    let view = view.clone();
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(2500)).await;
                            let _ = cx.update(|window, cx| view.update(cx, |v, cx| v.add_project(dir.into(), window, cx)));
                        })
                        .detach();
                }
                // Click as a person would, through the window's own event dispatch: `x,y` (window points),
                // `right:x,y`, `move:x,y`, `keys:cmd-n h i enter`, `wait:ms`, separated by `;`.
                if let Ok(spec) = std::env::var("LILA_APP_CLICK") {
                    let spec: &'static str = Box::leak(spec.into_boxed_str());
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(3000)).await;
                            for step in spec.split(';') {
                                if let Some(ms) = step.strip_prefix("wait:") {
                                    cx.background_executor().timer(std::time::Duration::from_millis(ms.parse().unwrap_or(500))).await;
                                    continue;
                                }
                                if let Some(keys) = step.strip_prefix("keys:") {
                                    for k in keys.split_whitespace().filter_map(|k| gpui::Keystroke::parse(k).ok()) {
                                        let _ = cx.update(|window, cx| window.dispatch_keystroke(k, cx));
                                        cx.background_executor().timer(std::time::Duration::from_millis(40)).await;
                                    }
                                    cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                                    continue;
                                }
                                if let Some(at) = step.strip_prefix("move:")
                                    && let Some((x, y)) = at.split_once(',').and_then(|(x, y)| Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?)))
                                {
                                    let position = point(px(x), px(y));
                                    let _ = cx.update(|window, cx| window.dispatch_event(gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent { position, pressed_button: None, modifiers: gpui::Modifiers::default() }), cx));
                                    cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                                    continue;
                                }
                                let (button, at) = step.strip_prefix("right:").map_or((gpui::MouseButton::Left, step), |r| (gpui::MouseButton::Right, r));
                                let Some((x, y)) = at.split_once(',').and_then(|(x, y)| Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?))) else { continue };
                                let position = point(px(x), px(y));
                                let _ = cx.update(|window, cx| {
                                    let m = gpui::Modifiers::default();
                                    window.dispatch_event(gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent { position, pressed_button: None, modifiers: m }), cx);
                                });
                                cx.background_executor().timer(std::time::Duration::from_millis(150)).await;
                                let _ = cx.update(|window, cx| {
                                    let m = gpui::Modifiers::default();
                                    window.dispatch_event(gpui::PlatformInput::MouseDown(gpui::MouseDownEvent { button, position, modifiers: m, click_count: 1, first_mouse: false }), cx);
                                    window.dispatch_event(gpui::PlatformInput::MouseUp(gpui::MouseUpEvent { button, position, modifiers: m, click_count: 1 }), cx);
                                });
                                cx.background_executor().timer(std::time::Duration::from_millis(700)).await;
                            }
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
                // Open the global settings: `usage` or `about`.
                if let Ok(page) = std::env::var("LILA_APP_PREFS") {
                    let view = view.clone();
                    let page = if page == "about" { crate::prefs::PrefPage::About } else { crate::prefs::PrefPage::Usage };
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                            let _ = cx.update(|_, cx| view.update(cx, |v, cx| v.open_prefs(page, cx)));
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
