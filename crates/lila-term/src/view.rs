//! One herdr-core terminal as a GPUI view: attach, draw, type, scroll, select.
//! Key encoding, IME handling and row painting follow herdr-app's terminal.rs.

use crate::hc::{Core, Stream};
use crate::vt::{self, CursorShape, Rgb, Select, Vt};
use ghostty_sys::*;
use gpui::prelude::*;
use gpui::*;
use serde_json::json;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

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
}

impl Grid {
    fn fit(&mut self, cols: u16, rows: u16, cell: Size<Pixels>, scale: f32) {
        self.cell = cell;
        let dev = device_cell(cell, scale);
        if (cols, rows) == (self.vt.cols, self.vt.rows) && dev == self.vt_cell {
            return;
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

pub struct TerminalView {
    pub address: String,
    grid: Rc<RefCell<Grid>>,
    focus: FocusHandle,
    /// Why the stream ended (exited, lost, attach error).
    ended: Option<String>,
    title: Option<String>,
    scroll_rest: f32,
    selecting: bool,
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
        }));
        let mut view = TerminalView { address, grid, focus: cx.focus_handle(), ended: None, title: None, scroll_rest: 0.0, selecting: false, _reader: None };
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
        cx.notify();
        open
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref().filter(|t| !t.is_empty())
    }

    pub fn ended(&self) -> Option<&str> {
        self.ended.as_deref()
    }

    fn key_down(&mut self, ev: &KeyDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
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

    fn scroll(&mut self, ev: &ScrollEvent, _w: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let mut g = self.grid.borrow_mut();
        let lines = match ev.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => f32::from(p.y) / f32::from(g.cell.height),
        };
        self.scroll_rest += lines;
        let n = self.scroll_rest.trunc() as isize;
        if n == 0 {
            return;
        }
        self.scroll_rest -= n as f32;
        if g.vt.alt_screen() {
            let key = if n > 0 { GHOSTTY_KEY_ARROW_UP } else { GHOSTTY_KEY_ARROW_DOWN };
            let mut out = Vec::new();
            for _ in 0..n.unsigned_abs() {
                out.extend(g.vt.encode_key(key, 0, 0, None, None));
            }
            g.send(&out);
        } else {
            g.vt.scroll_viewport(-n);
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
        let observe = self.grid.borrow().observe;
        let ended = self.ended.clone();
        div()
            .id(SharedString::from(format!("term-{}", self.address)))
            .size_full()
            .relative()
            .bg(hsla(vt::theme().bg))
            .track_focus(&self.focus)
            .when(!observe, |d| {
                d.on_key_down(cx.listener(Self::key_down))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus, cx);
                            this.selecting = true;
                            this.select(Select::Press, ev.position, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                        if this.selecting && ev.pressed_button == Some(MouseButton::Left) {
                            this.select(Select::Drag, ev.position, cx);
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseUpEvent, _, cx| {
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
                    move |bounds, window, _cx| {
                        let (cols, rows) = grid_size(bounds.size, cell);
                        grid.borrow_mut().fit(cols, rows, cell, window.scale_factor());
                    },
                    move |bounds, _, window, cx| {
                        paint_grid(bounds, &grid_paint, focused, window, cx);
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
    }
}

fn paint_grid(bounds: Bounds<Pixels>, grid: &Rc<RefCell<Grid>>, focused: bool, window: &mut Window, cx: &mut App) {
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

    window.paint_quad(fill(bounds, hsla(frame.bg)));
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
