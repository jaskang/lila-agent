//! One herdr-core terminal as a GPUI view: attach, draw, type, scroll, select.
//! Key encoding, IME handling and row painting follow herdr-app's terminal.rs.

use crate::hc::{Core, Stream};
use crate::links;
use crate::vt::{self, CursorShape, Rgb, Select, Vt};
use ghostty_sys::*;
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::{ActiveTheme, Disableable, IconName, Sizable, h_flex, v_flex};
use gpui::prelude::*;
use gpui::*;
use serde_json::json;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

fn hsla(c: Rgb) -> Hsla {
    Rgba { r: c.0 as f32 / 255.0, g: c.1 as f32 / 255.0, b: c.2 as f32 / 255.0, a: 1.0 }.into()
}

const PAD: f32 = 6.0;

struct Grid {
    vt: Vt,
    channel: Option<u32>,
    core: Arc<Core>,
    cell: Size<Pixels>,
    vt_cell: (u32, u32),
    origin: Point<Pixels>,
    /// What an input method is composing.
    marked: Option<String>,
    cursor_at: Option<Bounds<Pixels>>,
    observe: bool,
    scale: f32,
}

impl Grid {
    fn fit(&mut self, cols: u16, rows: u16, cell: Size<Pixels>, scale: f32) -> bool {
        self.cell = cell;
        self.scale = scale;
        let dev = device_cell(cell, scale);
        if (cols, rows) == (self.vt.cols, self.vt.rows) && dev == self.vt_cell {
            return false;
        }
        self.vt.resize(cols, rows, dev.0, dev.1);
        self.vt_cell = dev;
        if let Some(ch) = self.channel
            && !self.observe
        {
            self.core.resize(ch, cols, rows);
            let replies = self.vt.take_replies();
            self.core.automatic_input(ch, &replies);
        }
        true
    }

    fn send(&self, bytes: &[u8]) {
        if let (Some(ch), false) = (self.channel, self.observe) {
            self.core.input(ch, bytes);
        }
    }
}

fn device_cell(cell: Size<Pixels>, scale: f32) -> (u32, u32) {
    let px = |v: Pixels| (f32::from(v) * scale).round().max(1.0) as u32;
    (px(cell.width), px(cell.height))
}

fn cell_font(bold: bool, italic: bool) -> Font {
    let mut f = font(crate::theme::FONT);
    f.weight = if bold { FontWeight::BOLD } else { FontWeight::NORMAL };
    f.style = if italic { FontStyle::Italic } else { FontStyle::Normal };
    f
}

pub fn cell_size(window: &Window) -> Size<Pixels> {
    let ts = window.text_system();
    let id = ts.resolve_font(&cell_font(false, false));
    let fs = px(crate::theme::FONT_SIZE);
    let w = ts.advance(id, fs, 'M').map(|s| s.width).unwrap_or(fs * 0.6);
    let h = ts.ascent(id, fs) + ts.descent(id, fs).abs();
    let scale = window.scale_factor();
    size(w, px((f32::from(h) * 1.2 * scale).ceil() / scale))
}

fn grid_size(bounds: Size<Pixels>, cell: Size<Pixels>) -> (u16, u16) {
    let cols = ((f32::from(bounds.width) - 2.0 * PAD) / f32::from(cell.width)).floor().max(2.0) as u16;
    let rows = ((f32::from(bounds.height) - 2.0 * PAD) / f32::from(cell.height)).floor().max(1.0) as u16;
    (cols, rows)
}

/// The key context terminals render in.
pub const KEY_CONTEXT: &str = "Terminal";

actions!(lila_terminal, [Find, FindNext, FindPrevious, CloseFind]);

/// Keys the window binds that a terminal must get as typing instead. gpui-kit's
/// Root binds Tab / Shift-Tab to focus moves and ⌘C to copying its own text
/// selection; inside a terminal they go to the program (and ⌘C copies the
/// terminal's selection, see `key_down`). Find bindings cover both the grid
/// and its input, so the input's own editor search cannot take them over.
/// Call once, after `gpui_kit::init`.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", NoAction, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-tab", NoAction, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-c", NoAction, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-f", Find, Some("LilaTerminal")),
        KeyBinding::new("cmd-f", Find, Some("LilaTerminal > Input")),
        KeyBinding::new("cmd-g", FindNext, Some("LilaTerminal")),
        KeyBinding::new("cmd-g", FindNext, Some("LilaTerminal > Input")),
        KeyBinding::new("cmd-shift-g", FindPrevious, Some("LilaTerminal")),
        KeyBinding::new("cmd-shift-g", FindPrevious, Some("LilaTerminal > Input")),
        KeyBinding::new("escape", CloseFind, Some("TerminalSearch > Input")),
    ]);
}

pub struct TerminalView {
    pub address: String,
    grid: Rc<RefCell<Grid>>,
    focus: FocusHandle,
    /// Why the stream ended (exited, lost, attach error).
    ended: Option<String>,
    title: Option<String>,
    /// Touchpad movement not yet a whole line (per axis, reset at each gesture).
    scrolled: Point<f32>,
    selecting: bool,
    /// Relative file links start at the project's shared working directory.
    cwd: Option<PathBuf>,
    search_open: bool,
    search_input: Option<Entity<InputState>>,
    _search_subscription: Option<Subscription>,
    _search_work: Option<Task<()>>,
    pointer: Option<Point<Pixels>>,
    hover_cell: Option<(u16, u16)>,
    hovered: Option<(vt::LinkHit, links::Target)>,
    pressed_link: Option<(links::Target, Point<Pixels>)>,
    _reader: Option<Task<()>>,
}

impl TerminalView {
    /// Attaches to `address`. `observe`: watch only (thumbnails); never types or resizes.
    pub fn new(core: Arc<Core>, address: String, observe: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cell = cell_size(window);
        let vt = Vt::new(100, 30);
        let grid = Rc::new(RefCell::new(Grid {
            vt,
            channel: None,
            core: core.clone(),
            cell,
            vt_cell: (0, 0),
            origin: point(px(0.0), px(0.0)),
            marked: None,
            cursor_at: None,
            observe,
            scale: window.scale_factor(),
        }));
        let mut view = TerminalView {
            address, grid, focus: cx.focus_handle(), ended: None, title: None,
            scrolled: point(0.0, 0.0), selecting: false, cwd: None,
            search_open: false, search_input: None, _search_subscription: None, _search_work: None,
            pointer: None, hover_cell: None, hovered: None, pressed_link: None, _reader: None,
        };
        view.attach(window, cx);
        view
    }

    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let core = self.grid.borrow().core.clone();
        let address = self.address.clone();
        let (cols, rows) = (self.grid.borrow().vt.cols, self.grid.borrow().vt.rows);
        let theme = vt::theme();
        let hex = |c: Rgb| format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2);
        let dev = device_cell(cell_size(window), window.scale_factor());
        let display = json!({"fg": hex(theme.fg), "bg": hex(theme.bg), "cursor": hex(theme.cursor), "cell": [dev.0, dev.1]});
        let observe = self.grid.borrow().observe;
        self._reader = Some(cx.spawn(async move |this, cx| {
            let attached = cx
                .background_executor()
                .spawn(async move { core.attach(&address, cols, rows, display, observe) })
                .await;
            let rx = match attached {
                Ok(a) => {
                    let _ = this.update(cx, |this, cx| {
                        let mut g = this.grid.borrow_mut();
                        g.channel = Some(a.channel);
                        // The window laid out while the attach was in flight: the
                        // program still has the size we attached with.
                        if !g.observe && (g.vt.cols, g.vt.rows) != (cols, rows) {
                            g.core.resize(a.channel, g.vt.cols, g.vt.rows);
                        }
                        drop(g);
                        this.ended = None;
                        cx.notify();
                    });
                    a.stream
                }
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        this.ended = Some(format!("attach failed: {e:#}"));
                        cx.notify();
                    });
                    return;
                }
            };
            while let Ok(first) = rx.recv().await {
                let mut batch = vec![first];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                let alive = this.update(cx, |this, cx| this.ingest(batch, cx));
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn ingest(&mut self, batch: Vec<Stream>, cx: &mut Context<Self>) -> bool {
        let mut open = true;
        {
            let mut g = self.grid.borrow_mut();
            for item in batch {
                match item {
                    Stream::Data(bytes) => {
                        g.vt.write(&bytes);
                        let [clipboard, _primary] = g.vt.take_clipboard();
                        if let Some(text) = clipboard {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                        }
                        if let Some(ch) = g.channel {
                            g.core.credit(ch, bytes.len());
                            let replies = g.vt.take_replies();
                            if !g.observe {
                                g.core.automatic_input(ch, &replies);
                            }
                        }
                    }
                    Stream::Closed(why) => {
                        open = false;
                        g.channel = None;
                        self.ended = Some(why.get("reason").and_then(|r| r.as_str()).unwrap_or("ended").to_owned());
                    }
                    Stream::Lost => {
                        open = false;
                        g.channel = None;
                        self.ended = Some("connection to herdr-core lost".into());
                    }
                }
            }
            if let Some(t) = g.vt.take_title() {
                self.title = Some(t);
            }
        }
        self.clear_hover();
        if self.search_open {
            self.drive_search(cx);
        }
        cx.notify();
        open
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref().filter(|t| !t.is_empty())
    }

    pub fn ended(&self) -> Option<&str> {
        self.ended.as_deref()
    }

    /// Initial directory for relative file links (members currently share it).
    pub fn set_working_directory(&mut self, cwd: PathBuf) {
        self.cwd = Some(cwd);
    }

    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.grid.borrow().observe {
            return;
        }
        if self.search_input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索终端历史…"));
            self._search_subscription = Some(cx.subscribe_in(&input, window, |this, input, event, _, cx| match event {
                InputEvent::Change if this.search_open => {
                    this.grid.borrow_mut().vt.start_search(&input.read(cx).value());
                    this.drive_search(cx);
                    cx.notify();
                }
                InputEvent::PressEnter { shift, .. } => this.navigate_search(*shift, cx),
                _ => {}
            }));
            self.search_input = Some(input);
        }
        self.search_open = true;
        self.clear_hover();
        let input = self.search_input.as_ref().unwrap();
        self.grid.borrow_mut().vt.start_search(&input.read(cx).value());
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.drive_search(cx);
        cx.notify();
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_open = false;
        self._search_work = None;
        self.grid.borrow_mut().vt.clear_search();
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn navigate_search(&mut self, previous: bool, cx: &mut Context<Self>) {
        if self.search_open {
            self.grid.borrow_mut().vt.search_navigate(previous);
            self.clear_hover();
            cx.notify();
        }
    }

    /// Search incrementally; stop scheduling frames as soon as indexing ends.
    fn drive_search(&mut self, cx: &mut Context<Self>) {
        self._search_work = Some(cx.spawn(async move |this, cx| {
            loop {
                let more = this.update(cx, |this, cx| {
                    let mut g = this.grid.borrow_mut();
                    if g.vt.progress_search() {
                        cx.notify();
                    }
                    this.search_open && !g.vt.search_progress().complete
                });
                if !matches!(more, Ok(true)) {
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(16)).await;
            }
        }));
    }

    fn search_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.search_open {
            return None;
        }
        let input = self.search_input.as_ref()?;
        let progress = self.grid.borrow().vt.search_progress();
        let count = if input.read(cx).value().is_empty() {
            "输入关键词".to_owned()
        } else if !progress.complete {
            format!("搜索中 · {}", progress.total)
        } else if let Some(index) = progress.selected {
            format!("{} / {}", index + 1, progress.total)
        } else {
            format!("{} 处匹配", progress.total)
        };
        let disabled = progress.total == 0 || !progress.complete;
        Some(h_flex()
            .key_context("TerminalSearch")
            .h(px(38.)).flex_none().px_2().gap_1()
            .bg(cx.theme().background).border_b_1().border_color(cx.theme().border)
            .child(div().flex_1().min_w_0().child(Input::new(input).small()))
            .child(div().text_xs().text_color(cx.theme().muted_foreground).child(count))
            .child(Button::new("find-previous").ghost().xsmall().icon(IconName::ChevronUp).tooltip("上一处 (⇧Enter / ⇧⌘G)").disabled(disabled).on_click(cx.listener(|this, _, _, cx| this.navigate_search(true, cx))))
            .child(Button::new("find-next").ghost().xsmall().icon(IconName::ChevronDown).tooltip("下一处 (Enter / ⌘G)").disabled(disabled).on_click(cx.listener(|this, _, _, cx| this.navigate_search(false, cx))))
            .child(Button::new("find-close").ghost().xsmall().icon(IconName::Close).tooltip("关闭搜索 (Esc)").on_click(cx.listener(|this, _, window, cx| this.close_search(window, cx))))
            .into_any_element())
    }

    fn clear_hover(&mut self) {
        self.hovered = None;
        self.hover_cell = None;
    }

    fn link_at(&self, pos: Point<Pixels>) -> Option<(vt::LinkHit, links::Target)> {
        let g = self.grid.borrow();
        let x = f32::from(pos.x - g.origin.x);
        let y = f32::from(pos.y - g.origin.y);
        if x < 0.0 || y < 0.0 || x >= f32::from(g.cell.width) * g.vt.cols as f32 || y >= f32::from(g.cell.height) * g.vt.rows as f32 {
            return None;
        }
        let hit = g.vt.link_at((x / f32::from(g.cell.width)) as u16, (y / f32::from(g.cell.height)) as u16, |text, at, _| {
            let range = links::find(text, at)?;
            links::resolve(&text[range.clone()], self.cwd.as_deref()).map(|_| range)
        })?;
        let target = links::resolve(&hit.target, self.cwd.as_deref())?;
        Some((hit, target))
    }

    fn hover_link(&mut self, pos: Point<Pixels>, modifiers: Modifiers, cx: &mut Context<Self>) {
        self.pointer = Some(pos);
        if !link_modifier(modifiers) || self.selecting {
            if self.hovered.is_some() {
                self.clear_hover();
                cx.notify();
            }
            return;
        }
        let (col, row, x, y) = self.cell_at(pos);
        let g = self.grid.borrow();
        let inside = pos.x >= g.origin.x && pos.y >= g.origin.y
            && x < f32::from(g.cell.width) * g.vt.cols as f32 && y < f32::from(g.cell.height) * g.vt.rows as f32;
        drop(g);
        if !inside {
            self.clear_hover();
            cx.notify();
            return;
        }
        if self.hover_cell == Some((col, row)) {
            return;
        }
        self.hovered = self.link_at(pos);
        self.hover_cell = Some((col, row));
        cx.notify();
    }

    fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        if k.key == "escape" && self.search_open {
            self.close_search(window, cx);
            cx.stop_propagation();
            return;
        }
        if k.modifiers.platform {
            match k.key.as_str() {
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        let g = self.grid.borrow();
                        let bytes = g.vt.paste_bytes(&text);
                        g.send(&bytes);
                    }
                    cx.stop_propagation();
                }
                "c" => {
                    if let Some(text) = self.grid.borrow().vt.selection_text() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        cx.stop_propagation();
                    }
                }
                _ => {}
            }
            return;
        }
        if !k.key.starts_with("shift") && !k.key.starts_with("control") && !k.key.starts_with("alt") {
            self.grid.borrow_mut().vt.clear_selection();
        }
        let bytes = key_bytes(&mut self.grid.borrow_mut().vt, ev);
        if let Some(bytes) = bytes {
            // Typing jumps back to the live screen, as terminals do.
            self.grid.borrow_mut().vt.scroll_to_bottom();
            self.grid.borrow().send(&bytes);
            cx.stop_propagation();
        }
    }

    /// Wheel and touchpad, as herdr-app (and Ghostty) scroll: a program that tracks
    /// the mouse gets wheel buttons, a full-screen one arrow keys, and the
    /// shell's screen scrolls its history.
    fn scroll(&mut self, ev: &ScrollEvent, _w: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.clear_hover();
        let mut g = self.grid.borrow_mut();
        let (cell, cols, rows) = (g.cell, g.vt.cols, g.vt.rows);
        let steps = scroll_delta_steps(&mut self.scrolled, ev.delta, ev.touch_phase, cell, cols, rows);
        let n = steps.y;
        if steps.x == 0 && n == 0 {
            return;
        }
        if g.vt.mouse_tracking() {
            let ((x, y), size) = mouse_at(&g, ev.position);
            let out = scroll_mouse_bytes(&mut g.vt, steps, x, y, size);
            g.send(&out);
        } else if n == 0 {
            return;
        } else if g.vt.alt_screen() {
            let key = if n > 0 { GHOSTTY_KEY_ARROW_UP } else { GHOSTTY_KEY_ARROW_DOWN };
            let mut out = Vec::new();
            for _ in 0..n.unsigned_abs() {
                out.extend(g.vt.encode_key(key, 0, 0, None, None));
            }
            g.send(&out);
        } else {
            g.vt.scroll_viewport(-(n as isize));
        }
        drop(g);
        if self.search_open {
            self.drive_search(cx);
        }
        cx.notify();
    }

    fn cell_at(&self, pos: Point<Pixels>) -> (u16, u16, f32, f32) {
        let g = self.grid.borrow();
        let (x, y) = (f32::from(pos.x - g.origin.x).max(0.0), f32::from(pos.y - g.origin.y).max(0.0));
        let (col, row) = ((x / f32::from(g.cell.width)) as u16, (y / f32::from(g.cell.height)) as u16);
        (col.min(g.vt.cols.saturating_sub(1)), row.min(g.vt.rows.saturating_sub(1)), x, y)
    }

    fn select(&mut self, step: Select, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let (col, row, x, y) = self.cell_at(pos);
        let mut g = self.grid.borrow_mut();
        let (w, h) = (f32::from(g.cell.width), f32::from(g.cell.height));
        g.vt.select(step, col, row, x, y, w, h);
        cx.notify();
    }
}

// ---- scrolling (herdr-app's terminal.rs) ----

fn scroll_delta_steps(scrolled: &mut Point<f32>, delta: ScrollDelta, phase: TouchPhase, cell: Size<Pixels>, cols: u16, rows: u16) -> Point<i32> {
    let lines = match delta {
        // As in Ghostty, a sideways notch moves one column; a vertical one, three lines.
        ScrollDelta::Lines(p) if cfg!(target_os = "macos") => return point(macos_wheel_steps(p.x) / 3, macos_wheel_steps(p.y)),
        ScrollDelta::Lines(p) => p,
        ScrollDelta::Pixels(p) => point(p.x / cell.width, p.y / cell.height),
    };
    point(scroll_steps(&mut scrolled.x, phase, lines.x, cols as f32), scroll_steps(&mut scrolled.y, phase, lines.y, rows as f32))
}

/// Whole lines for one touchpad event: small moves add up to a line before one
/// scrolls, each gesture starts from nothing, and it stays under a screen so
/// turning back is quick.
fn scroll_steps(scrolled: &mut f32, phase: TouchPhase, lines: f32, rows: f32) -> i32 {
    match phase {
        TouchPhase::Started => {
            *scrolled = 0.0;
            0
        }
        TouchPhase::Moved => {
            let before = scrolled.trunc() as i32;
            *scrolled += lines;
            let after = scrolled.trunc() as i32;
            *scrolled %= rows.max(1.0);
            after - before
        }
        _ => 0,
    }
}

/// Lines for a macOS mouse wheel event, as Ghostty scrolls them: macOS reports a
/// slow notch as a fraction of a tick (often 0.1), so an event counts as at least
/// one tick, times Ghostty's default multiplier of 3.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn macos_wheel_steps(ticks: f32) -> i32 {
    if ticks == 0.0 {
        return 0;
    }
    (ticks.signum() * ticks.abs().max(1.0) * 3.0).trunc() as i32
}

fn scroll_mouse_bytes(vt: &mut Vt, steps: Point<i32>, x: f32, y: f32, size: (u32, u32, f32, f32)) -> Vec<u8> {
    let mut out = Vec::new();
    for (steps, positive, negative) in [(steps.y, GHOSTTY_MOUSE_BUTTON_FOUR, GHOSTTY_MOUSE_BUTTON_FIVE), (steps.x, GHOSTTY_MOUSE_BUTTON_SIX, GHOSTTY_MOUSE_BUTTON_SEVEN)] {
        let button = if steps > 0 { positive } else { negative };
        for _ in 0..steps.unsigned_abs() {
            out.extend(vt.encode_mouse(GHOSTTY_MOUSE_ACTION_PRESS, Some(button), 0, x, y, size, false));
        }
    }
    out
}

/// A pointer position and the grid for mouse reports, in device pixels.
fn mouse_at(g: &Grid, p: Point<Pixels>) -> ((f32, f32), (u32, u32, f32, f32)) {
    let s = g.scale;
    let (x, y) = (f32::from(p.x - g.origin.x).max(0.0), f32::from(p.y - g.origin.y).max(0.0));
    ((x * s, y * s), (g.vt.cols as u32, g.vt.rows as u32, f32::from(g.cell.width) * s, f32::from(g.cell.height) * s))
}


/// What a key press sends to the program: the key described to libghostty,
/// which encodes it in the mode the program asked for (herdr-app's key_bytes,
/// without the Windows and Option-as-Alt branches).
fn key_bytes(vt: &mut Vt, ev: &KeyDownEvent) -> Option<Vec<u8>> {
    let k = &ev.keystroke;
    let m = k.modifiers;
    if vt.kitty_keyboard_flags() == 0 && m.control && !m.alt && !m.platform && !m.function {
        let alias = match k.key.as_str() {
            "[" if !m.shift => Some((GHOSTTY_KEY_BRACKET_LEFT, GHOSTTY_KEY_ESCAPE)),
            "i" if !m.shift => Some((GHOSTTY_KEY_I, GHOSTTY_KEY_TAB)),
            "m" if !m.shift => Some((GHOSTTY_KEY_M, GHOSTTY_KEY_ENTER)),
            _ => None,
        };
        if let Some((key, alias)) = alias {
            let legacy = vt.encode_key(alias, 0, 0, None, None);
            return Some(control_alias_bytes(vt, key, &k.key, false, legacy));
        }
        if matches!(k.key.as_str(), "-" | "_") {
            let shifted = m.shift || k.key == "_";
            let text = if shifted { "_" } else { "-" };
            return Some(control_alias_bytes(vt, GHOSTTY_KEY_MINUS, text, shifted, vec![0x1f]));
        }
    }
    let Some((key, shifted, unshifted)) = map_key(&k.key) else {
        return k.key_char.as_ref().map(|text| text.as_bytes().to_vec());
    };
    let mut mods: GhosttyMods = 0;
    if m.shift || shifted {
        mods |= GHOSTTY_MODS_SHIFT as GhosttyMods;
    }
    if m.control {
        mods |= GHOSTTY_MODS_CTRL as GhosttyMods;
    }
    if m.alt {
        mods |= GHOSTTY_MODS_ALT as GhosttyMods;
    }
    let text = match k.key.as_str() {
        "space" => Some(" ".to_owned()),
        _ => k.key_char.clone(),
    };
    let mut consumed = mods & GHOSTTY_MODS_SHIFT as GhosttyMods;
    // macOS turns Option into text (Option as Alt is not offered yet).
    if m.alt && text.as_deref().is_some_and(|t| !t.is_empty()) {
        consumed |= GHOSTTY_MODS_ALT as GhosttyMods;
    }
    Some(vt.encode_key(key, mods, consumed, text.as_deref(), unshifted))
}

fn control_alias_bytes(vt: &mut Vt, key: GhosttyKey, text: &str, shifted: bool, legacy: Vec<u8>) -> Vec<u8> {
    let mods = GHOSTTY_MODS_CTRL as GhosttyMods | if shifted { GHOSTTY_MODS_SHIFT as GhosttyMods } else { 0 };
    let encoded = vt.encode_key(key, mods, 0, Some(text), text.chars().next());
    if encoded.starts_with(b"\x1b[27;") { encoded } else { legacy }
}

fn map_key(key: &str) -> Option<(GhosttyKey, bool, Option<char>)> {
    let named = match key {
        "enter" => GHOSTTY_KEY_ENTER,
        "tab" => GHOSTTY_KEY_TAB,
        "escape" => GHOSTTY_KEY_ESCAPE,
        "backspace" => GHOSTTY_KEY_BACKSPACE,
        "space" => return Some((GHOSTTY_KEY_SPACE, false, Some(' '))),
        "delete" => GHOSTTY_KEY_DELETE,
        "insert" => GHOSTTY_KEY_INSERT,
        "home" => GHOSTTY_KEY_HOME,
        "end" => GHOSTTY_KEY_END,
        "pageup" => GHOSTTY_KEY_PAGE_UP,
        "pagedown" => GHOSTTY_KEY_PAGE_DOWN,
        "up" => GHOSTTY_KEY_ARROW_UP,
        "down" => GHOSTTY_KEY_ARROW_DOWN,
        "left" => GHOSTTY_KEY_ARROW_LEFT,
        "right" => GHOSTTY_KEY_ARROW_RIGHT,
        _ => GHOSTTY_KEY_UNIDENTIFIED,
    };
    if named != GHOSTTY_KEY_UNIDENTIFIED {
        return Some((named, false, None));
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
        && (1..=12).contains(&n)
    {
        return Some((GHOSTTY_KEY_F1 + (n - 1) as GhosttyKey, false, None));
    }
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return None };
    const SHIFTED: &[(char, char)] = &[
        ('~', '`'), ('!', '1'), ('@', '2'), ('#', '3'), ('$', '4'), ('%', '5'), ('^', '6'), ('&', '7'),
        ('*', '8'), ('(', '9'), (')', '0'), ('_', '-'), ('+', '='), ('{', '['), ('}', ']'), ('|', '\\'),
        (':', ';'), ('"', '\''), ('<', ','), ('>', '.'), ('?', '/'),
    ];
    let (base, shifted) = match SHIFTED.iter().find(|(s, _)| *s == c) {
        Some((_, b)) => (*b, true),
        None => (c, false),
    };
    let code = match base {
        'a'..='z' => GHOSTTY_KEY_A + (base as u8 - b'a') as GhosttyKey,
        '0'..='9' => GHOSTTY_KEY_DIGIT_0 + (base as u8 - b'0') as GhosttyKey,
        '`' => GHOSTTY_KEY_BACKQUOTE,
        '\\' => GHOSTTY_KEY_BACKSLASH,
        '[' => GHOSTTY_KEY_BRACKET_LEFT,
        ']' => GHOSTTY_KEY_BRACKET_RIGHT,
        ',' => GHOSTTY_KEY_COMMA,
        '=' => GHOSTTY_KEY_EQUAL,
        '-' => GHOSTTY_KEY_MINUS,
        '.' => GHOSTTY_KEY_PERIOD,
        '\'' => GHOSTTY_KEY_QUOTE,
        ';' => GHOSTTY_KEY_SEMICOLON,
        '/' => GHOSTTY_KEY_SLASH,
        _ => GHOSTTY_KEY_UNIDENTIFIED,
    };
    Some((code, shifted, Some(base)))
}

/// Input methods (Pinyin, Japanese, dead keys) type here; finished text goes to the program.
impl EntityInputHandler for TerminalView {
    fn text_for_range(&mut self, range: std::ops::Range<usize>, adjusted: &mut Option<std::ops::Range<usize>>, _w: &mut Window, _cx: &mut Context<Self>) -> Option<String> {
        let marked = self.grid.borrow().marked.clone().unwrap_or_default();
        let units: Vec<u16> = marked.encode_utf16().collect();
        let range = range.start.min(units.len())..range.end.min(units.len());
        *adjusted = Some(range.clone());
        Some(String::from_utf16_lossy(&units[range]))
    }

    fn selected_text_range(&mut self, _ignore_disabled: bool, _w: &mut Window, _cx: &mut Context<Self>) -> Option<UTF16Selection> {
        let len = self.grid.borrow().marked.as_ref().map_or(0, |m| m.encode_utf16().count());
        Some(UTF16Selection { range: len..len, reversed: false })
    }

    fn marked_text_range(&self, _w: &mut Window, _cx: &mut Context<Self>) -> Option<std::ops::Range<usize>> {
        self.grid.borrow().marked.as_ref().filter(|m| !m.is_empty()).map(|m| 0..m.encode_utf16().count())
    }

    fn unmark_text(&mut self, _w: &mut Window, cx: &mut Context<Self>) {
        self.grid.borrow_mut().marked = None;
        cx.notify();
    }

    fn replace_text_in_range(&mut self, _range: Option<std::ops::Range<usize>>, text: &str, _w: &mut Window, cx: &mut Context<Self>) {
        self.grid.borrow_mut().marked = None;
        self.grid.borrow().send(text.as_bytes());
        cx.notify();
    }

    fn replace_and_mark_text_in_range(&mut self, _range: Option<std::ops::Range<usize>>, text: &str, _selected: Option<std::ops::Range<usize>>, _w: &mut Window, cx: &mut Context<Self>) {
        self.grid.borrow_mut().marked = (!text.is_empty()).then(|| text.to_owned());
        cx.notify();
    }

    fn bounds_for_range(&mut self, range: std::ops::Range<usize>, _element: Bounds<Pixels>, _w: &mut Window, _cx: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let g = self.grid.borrow();
        let at = g.cursor_at?;
        Some(Bounds::new(at.origin + point(g.cell.width * range.start as f32, px(0.0)), at.size))
    }

    fn character_index_for_point(&mut self, _point: Point<Pixels>, _w: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        None
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let cell = cell_size(window);
        let grid = self.grid.clone();
        let grid_paint = self.grid.clone();
        let input_view = cx.entity();
        let input_focus = self.focus.clone();
        let resize_view = cx.entity().downgrade();
        let observe = self.grid.borrow().observe;
        let ended = self.ended.clone();
        let search = self.search_bar(cx);
        let hovered = self.hovered.clone();
        let link_spans = hovered.as_ref().map(|(hit, _)| hit.spans.clone()).unwrap_or_default();
        let terminal = div()
            .id(SharedString::from(format!("term-{}", self.address)))
            .flex_1().min_h_0().w_full()
            .relative()
            .when(!crate::theme::transparent(), |d| d.bg(hsla(vt::theme().bg)))
            .cursor(if hovered.is_some() { CursorStyle::PointingHand } else { CursorStyle::IBeam })
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .when(!observe, |d| {
                d.on_key_down(cx.listener(Self::key_down))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus, cx);
                            if link_modifier(ev.modifiers)
                                && let Some((_, target)) = this.link_at(ev.position)
                            {
                                this.selecting = false;
                                this.pressed_link = Some((target, ev.position));
                                cx.stop_propagation();
                                return;
                            }
                            this.pressed_link = None;
                            this.selecting = true;
                            this.select(Select::Press, ev.position, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                        if this.selecting && ev.pressed_button == Some(MouseButton::Left) {
                            this.select(Select::Drag, ev.position, cx);
                        } else {
                            this.hover_link(ev.position, ev.modifiers, cx);
                        }
                    }))
                    .on_modifiers_changed(cx.listener(|this, ev: &ModifiersChangedEvent, _, cx| {
                        if let Some(pos) = this.pointer {
                            this.hover_cell = None;
                            this.hover_link(pos, ev.modifiers, cx);
                        }
                    }))
                    .on_hover(cx.listener(|this, hovered, _, cx| {
                        if !hovered {
                            this.pointer = None;
                            this.clear_hover();
                            cx.notify();
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                            if let Some((target, start)) = this.pressed_link.take() {
                                if link_modifier(ev.modifiers) && short_click(start, ev.position)
                                    && this.link_at(ev.position).is_some_and(|(_, at)| at == target)
                                {
                                    links::open(&target);
                                }
                                cx.stop_propagation();
                                return;
                            }
                            if this.selecting {
                                this.selecting = false;
                                this.select(Select::Release, ev.position, cx);
                            }
                        }),
                    )
            })
            .on_scroll(cx.listener(Self::scroll))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let (cols, rows) = grid_size(bounds.size, cell);
                        if grid.borrow_mut().fit(cols, rows, cell, window.scale_factor()) {
                            let view = resize_view.clone();
                            cx.defer(move |cx| {
                                let _ = view.update(cx, |this, cx| {
                                    this.clear_hover();
                                    if this.search_open {
                                        this.drive_search(cx);
                                    }
                                    cx.notify();
                                });
                            });
                        }
                    },
                    move |bounds, _, window, cx| {
                        paint_grid(bounds, &grid_paint, focused, &link_spans, window, cx);
                        if focused {
                            window.handle_input(&input_focus, ElementInputHandler::new(bounds, input_view), cx);
                        }
                    },
                )
                .size_full(),
            )
            .when_some(ended, |d, why| {
                d.child(
                    div()
                        .absolute()
                        .bottom_2()
                        .right_3()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(hsla(Rgb(0x32, 0x38, 0x44)))
                        .text_color(hsla(vt::theme().fg))
                        .text_xs()
                        .child(format!("[{why}]")),
                )
            })
            .when_some(hovered, |d, (hit, _)| {
                d.child(div().absolute().bottom_0().left_0().max_w_full().px_2().py_1().text_xs().truncate()
                    .bg(hsla(Rgb(0x32, 0x38, 0x44))).text_color(hsla(vt::theme().fg)).child(hit.target))
            });
        v_flex()
            .size_full().min_h_0()
            .key_context("LilaTerminal")
            .on_action(cx.listener(|this, _: &Find, window, cx| this.open_search(window, cx)))
            .on_action(cx.listener(|this, _: &FindNext, _, cx| this.navigate_search(false, cx)))
            .on_action(cx.listener(|this, _: &FindPrevious, _, cx| this.navigate_search(true, cx)))
            .on_action(cx.listener(|this, _: &CloseFind, window, cx| this.close_search(window, cx)))
            .children(search)
            .child(terminal)
    }
}

fn link_modifier(m: Modifiers) -> bool {
    m.platform && !m.control && !m.alt && !m.shift
}

fn short_click(start: Point<Pixels>, end: Point<Pixels>) -> bool {
    (f32::from(start.x - end.x)).abs() <= 4.0 && (f32::from(start.y - end.y)).abs() <= 4.0
}

fn paint_grid(bounds: Bounds<Pixels>, grid: &Rc<RefCell<Grid>>, focused: bool, link_spans: &vt::Spans, window: &mut Window, cx: &mut App) {
    let mut g = grid.borrow_mut();
    let cell = g.cell;
    let scale = window.scale_factor();
    let snap = |v: Pixels| px((f32::from(v) * scale).round() / scale);
    let origin = bounds.origin + point(px(PAD), px(PAD));
    let origin = point(snap(origin.x), snap(origin.y));
    g.origin = origin;
    let frame = g.vt.frame();
    let marked = g.marked.clone();
    drop(g);

    if !crate::theme::transparent() {
        window.paint_quad(fill(bounds, hsla(frame.bg)));
    }
    let at = |x: usize, y: usize| origin + point(cell.width * x as f32, cell.height * y as f32);
    let snap_box = |b: Bounds<Pixels>| {
        let (x0, x1) = (snap(b.origin.x), snap(b.origin.x + b.size.width));
        let (y0, y1) = (snap(b.origin.y), snap(b.origin.y + b.size.height));
        Bounds::new(point(x0, y0), size(x1 - x0, y1 - y0))
    };
    let font_size = px(crate::theme::FONT_SIZE);

    // Backgrounds first, merged into runs of one color.
    for (y, row) in frame.rows.iter().enumerate() {
        let mut x = 0;
        while x < row.len() {
            let Some(bg) = row[x].bg else {
                x += 1;
                continue;
            };
            let start = x;
            while x < row.len() && row[x].bg == Some(bg) {
                x += 1;
            }
            window.paint_quad(fill(snap_box(Bounds::new(at(start, y), size(cell.width * (x - start) as f32, cell.height))), hsla(bg)));
        }
    }
    for span in &frame.search_spans {
        let color = if span.selected { Rgb(0x85, 0x5c, 0x22) } else { Rgb(0x54, 0x48, 0x22) };
        window.paint_quad(fill(snap_box(Bounds::new(at(span.start as usize, span.row as usize), size(cell.width * (span.end - span.start + 1) as f32, cell.height))), hsla(color)));
    }
    // Text in runs of one style; box drawing as sprites; wide characters alone.
    for (y, row) in frame.rows.iter().enumerate() {
        let mut x = 0;
        while x < row.len() {
            let start = x;
            let c = &row[start];
            x += 1;
            if c.text.is_empty() && !c.underline && !c.strike {
                continue;
            }
            let mut color = hsla(c.fg);
            if c.faint {
                color.a = 0.6;
            }
            if crate::sprites::is_sprite(&c.text) {
                crate::sprites::paint(&c.text, Bounds::new(at(start, y), cell), color, window);
                continue;
            }
            let mut text = if c.text.is_empty() { " ".to_owned() } else { c.text.clone() };
            if !c.wide {
                while x < row.len() {
                    let d = &row[x];
                    let same = !d.wide && d.fg == c.fg && d.bold == c.bold && d.italic == c.italic && d.faint == c.faint && d.underline == c.underline && d.strike == c.strike;
                    if !same || (d.text.is_empty() && !d.underline && !d.strike) || crate::sprites::is_sprite(&d.text) || !d.text.is_ascii() {
                        break;
                    }
                    text.push_str(if d.text.is_empty() { " " } else { &d.text });
                    x += 1;
                }
            }
            if text.trim().is_empty() && !c.underline && !c.strike {
                continue;
            }
            let run = TextRun {
                len: text.len(),
                font: cell_font(c.bold, c.italic),
                color,
                background_color: None,
                underline: c.underline.then(|| UnderlineStyle { thickness: px(1.0), color: Some(color), wavy: false }),
                strikethrough: c.strike.then(|| StrikethroughStyle { thickness: px(1.0), color: Some(color) }),
            };
            let force = (!c.wide).then_some(cell.width);
            let line = window.text_system().shape_line(text.into(), font_size, &[run], force);
            let _ = line.paint(at(start, y), cell.height, TextAlign::Left, None, window, cx);
        }
    }

    for &(row, start, end) in link_spans {
        let p = at(start as usize, row as usize) + point(px(0.), cell.height - px(1.));
        window.paint_quad(fill(snap_box(Bounds::new(p, size(cell.width * (end - start + 1) as f32, px(1.)))), hsla(vt::theme().fg)));
    }

    if let Some(cur) = frame.cursor {
        let (x, y) = (cur.x as usize, cur.y as usize);
        let wide = frame.rows.get(y).and_then(|r| r.get(x)).is_some_and(|c| c.wide);
        let w = cell.width * if wide { 2.0 } else { 1.0 };
        let color = hsla(cur.color);
        let p = at(x, y);
        match (focused, cur.shape) {
            (true, CursorShape::Block) => {
                window.paint_quad(fill(snap_box(Bounds::new(p, size(w, cell.height))), color));
                if let Some(c) = frame.rows.get(y).and_then(|r| r.get(x)).filter(|c| !c.text.is_empty()) {
                    let run = TextRun { len: c.text.len(), font: cell_font(c.bold, c.italic), color: hsla(frame.bg), background_color: None, underline: None, strikethrough: None };
                    let line = window.text_system().shape_line(c.text.clone().into(), font_size, &[run], None);
                    let _ = line.paint(p, cell.height, TextAlign::Left, None, window, cx);
                }
            }
            (true, CursorShape::Bar) => window.paint_quad(fill(snap_box(Bounds::new(p, size(px(2.0), cell.height))), color)),
            (true, CursorShape::Underline) => window.paint_quad(fill(snap_box(Bounds::new(p + point(px(0.0), cell.height - px(2.0)), size(w, px(2.0)))), color)),
            _ => window.paint_quad(outline(snap_box(Bounds::new(p, size(w, cell.height))), color, BorderStyle::Solid)),
        }
    }
    if let (Some((cx_, cy_)), Some(text)) = (frame.cursor_cell, marked.filter(|t| !t.is_empty())) {
        let p = at(cx_ as usize, cy_ as usize);
        let fg = hsla(vt::theme().fg);
        let run = TextRun { len: text.len(), font: cell_font(false, false), color: fg, background_color: None, underline: Some(UnderlineStyle { thickness: px(1.0), color: Some(fg), wavy: false }), strikethrough: None };
        let line = window.text_system().shape_line(text.into(), font_size, &[run], None);
        window.paint_quad(fill(snap_box(Bounds::new(p, size(line.width, cell.height))), hsla(frame.bg)));
        let _ = line.paint(p, cell.height, TextAlign::Left, None, window, cx);
    }
    grid.borrow_mut().cursor_at = frame.cursor_cell.map(|(x, y)| Bounds::new(at(x as usize, y as usize), cell));
}

#[cfg(test)]
mod interaction_tests {
    use super::{Core, Stream, TerminalView, links};
    use gpui::{AnyWindowHandle, AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px, size};
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    fn terminal(cx: &mut TestAppContext, text: &[u8]) -> (Entity<TerminalView>, AnyWindowHandle, Receiver<Vec<u8>>) {
        cx.update(|cx| {
            gpui::init(cx);
            super::init(cx);
        });
        let (core, frames) = Core::for_test();
        let mut view = None;
        let window = cx.add_window(|window, cx| {
            let terminal = cx.new(|cx| {
                let mut view = TerminalView::new(core, "test".into(), false, window, cx);
                // Cancel attach before its future runs; keep the real view,
                // VT and input dispatch, with only the transport replaced.
                view._reader = None;
                view.grid.borrow_mut().channel = Some(7);
                view.grid.borrow_mut().vt.write(text);
                view
            });
            let focus = terminal.read(cx).focus.clone();
            window.focus(&focus, cx);
            view = Some(terminal.clone());
            gpui::base::Root::new(terminal, window, cx)
        });
        (view.unwrap(), window.into(), frames)
    }

    #[gpui::test]
    fn search_keys_and_text_stay_out_of_the_terminal(cx: &mut TestAppContext) {
        let (view, window, frames) = terminal(cx, b"needle one\r\nneedle two\r\nother");
        let mut cx = VisualTestContext::from_window(window, cx);
        cx.simulate_keystrokes("cmd-f");
        cx.simulate_input("needle");
        cx.run_until_parked();
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert!(view.search_open);
            assert_eq!(view.grid.borrow().vt.search_progress().total, 2);
        });
        for (keys, selected) in [("enter", 0), ("cmd-g", 1), ("cmd-shift-g", 0), ("shift-enter", 1)] {
            cx.simulate_keystrokes(keys);
            cx.update(|_, cx| assert_eq!(view.read(cx).grid.borrow().vt.search_progress().selected, Some(selected)));
        }
        cx.update(|_, cx| view.update(cx, |view, cx| {
            view.ingest(vec![Stream::Data(b"\r\nneedle three".to_vec())], cx);
        }));
        cx.simulate_resize(size(px(600.), px(400.)));
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        cx.update(|_, cx| {
            let view = view.read(cx);
            let progress = view.grid.borrow().vt.search_progress();
            assert!(progress.complete);
            assert_eq!(progress.total, 3);
        });
        cx.simulate_keystrokes("cmd-f");
        cx.simulate_input("other");
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(view.read(cx).grid.borrow().vt.search_progress().total, 1));
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert!(!view.search_open);
            assert!(view.focus.is_focused(window));
            assert_eq!(view.grid.borrow().vt.search_progress().total, 0);
        });
        assert!(frames.try_iter().all(|frame| frame[4] != crate::hc::DATA), "search must never type into the CLI");
        cx.simulate_keystrokes("x tab escape");
        let typed: Vec<u8> = frames.try_iter().filter(|frame| frame[4] == crate::hc::DATA).flat_map(|frame| frame[9..].to_vec()).collect();
        assert_eq!(typed, b"x\t\x1b");
        cx.simulate_keystrokes("cmd-f");
        cx.run_until_parked();
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert_eq!(view.search_input.as_ref().unwrap().read(cx).value().as_ref(), "other");
        });
    }

    #[gpui::test]
    fn command_hover_resolves_links_and_leaves_plain_clicks_for_selection(cx: &mut TestAppContext) {
        let (view, window, _) = terminal(cx, b"https://example.com\r\n\x1b]8;;https://example.org/target\x1b\\named link\x1b]8;;\x1b\\");
        let mut cx = VisualTestContext::from_window(window, cx);
        cx.run_until_parked();
        let at = cx.update(|_, cx| {
            let view = view.read(cx);
            let g = view.grid.borrow();
            g.origin + point(g.cell.width * 2.0, g.cell.height * 0.5)
        });
        let cmd = Modifiers { platform: true, ..Default::default() };
        cx.simulate_mouse_move(at, None, cmd);
        cx.update(|_, cx| {
            assert_eq!(view.read(cx).hovered.as_ref().map(|(_, target)| target.clone()), Some(links::Target::Url("https://example.com".into())));
        });
        cx.simulate_modifiers_change(Modifiers::default());
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert!(view.hovered.is_none());
            assert!(view.selecting);
            assert!(view.pressed_link.is_none());
        });
        cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        cx.update(|_, cx| {
            let view = view.read(cx);
            let g = view.grid.borrow();
            let second_row = at + point(px(0.), g.cell.height);
            assert_eq!(view.link_at(second_row).unwrap().1, links::Target::Url("https://example.org/target".into()));
            assert!(view.link_at(g.origin - point(px(1.), px(1.))).is_none());
        });
        // A command drag cancels opening, so verification launches no browser.
        cx.simulate_mouse_down(at, MouseButton::Left, cmd);
        cx.update(|_, cx| assert!(view.read(cx).pressed_link.is_some()));
        cx.simulate_mouse_up(at + point(px(20.), px(0.)), MouseButton::Left, cmd);
        cx.update(|_, cx| assert!(view.read(cx).pressed_link.is_none()));
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::{macos_wheel_steps, scroll_steps};
    use gpui::TouchPhase;

    #[test]
    fn a_slow_wheel_notch_still_scrolls_three_lines() {
        assert_eq!(macos_wheel_steps(0.1), 3);
        assert_eq!(macos_wheel_steps(-0.1), -3);
        assert_eq!(macos_wheel_steps(2.0), 6);
        assert_eq!(macos_wheel_steps(0.0), 0);
    }

    #[test]
    fn touchpad_moves_add_up_to_lines() {
        let mut acc = 0.0;
        assert_eq!(scroll_steps(&mut acc, TouchPhase::Started, 0.0, 40.0), 0);
        assert_eq!(scroll_steps(&mut acc, TouchPhase::Moved, 0.4, 40.0), 0);
        assert_eq!(scroll_steps(&mut acc, TouchPhase::Moved, 0.4, 40.0), 0);
        assert_eq!(scroll_steps(&mut acc, TouchPhase::Moved, 0.4, 40.0), 1);
        assert_eq!(scroll_steps(&mut acc, TouchPhase::Moved, -2.5, 40.0), -2);
        assert_eq!(scroll_steps(&mut acc, TouchPhase::Ended, 5.0, 40.0), 0);
    }
}
