#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals, dead_code, clippy::all)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

// Style flags are C macros with casts, which bindgen leaves out (style.h).
pub const GHOSTTY_STYLE_FLAG_BOLD: GhosttyStyleFlags = 1 << 0;
pub const GHOSTTY_STYLE_FLAG_ITALIC: GhosttyStyleFlags = 1 << 1;
pub const GHOSTTY_STYLE_FLAG_FAINT: GhosttyStyleFlags = 1 << 2;
pub const GHOSTTY_STYLE_FLAG_BLINK: GhosttyStyleFlags = 1 << 3;
pub const GHOSTTY_STYLE_FLAG_INVERSE: GhosttyStyleFlags = 1 << 4;
pub const GHOSTTY_STYLE_FLAG_INVISIBLE: GhosttyStyleFlags = 1 << 5;
pub const GHOSTTY_STYLE_FLAG_STRIKETHROUGH: GhosttyStyleFlags = 1 << 6;
pub const GHOSTTY_STYLE_FLAG_OVERLINE: GhosttyStyleFlags = 1 << 7;
pub const GHOSTTY_STYLE_UNDERLINE_MASK: GhosttyStyleFlags = 0x7 << GHOSTTY_STYLE_UNDERLINE_SHIFT;

impl GhosttyStyle {
    pub fn has(&self, flag: GhosttyStyleFlags) -> bool {
        self.flags & flag != 0
    }

    /// GHOSTTY_STYLE_UNDERLINE(): a GhosttySgrUnderline value, 0 for none.
    pub fn underline(&self) -> u32 {
        u32::from((self.flags & GHOSTTY_STYLE_UNDERLINE_MASK) >> GHOSTTY_STYLE_UNDERLINE_SHIFT)
    }
}
