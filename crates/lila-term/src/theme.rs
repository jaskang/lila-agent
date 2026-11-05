//! Terminal colors: Atom One Dark, as herdr-app ships by default.

use crate::vt::{Rgb, Theme};

const fn rgb(hex: u32) -> Rgb {
    Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub const BACKGROUND: u32 = 0x21252b;
pub const FOREGROUND: u32 = 0xabb2bf;

pub fn terminal() -> Theme {
    let ansi = [
        0x454c59, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, 0x767676, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd,
        0x56b6c2, 0xabb2bf,
    ];
    Theme {
        fg: rgb(FOREGROUND),
        bg: rgb(BACKGROUND),
        cursor: rgb(FOREGROUND),
        selection_bg: rgb(0x323844),
        palette16: ansi.map(rgb),
        minimum_contrast: 1.0,
    }
}

/// The monospace font and size cells are measured in.
pub const FONT: &str = "Menlo";
pub const FONT_SIZE: f32 = 13.0;
