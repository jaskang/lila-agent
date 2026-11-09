//! A herdr-core terminal for GPUI. `hc` and `vt` (with `sprites`, `links`,
//! `win32_keys`) are ported from herdr-app (Apache-2.0); `view` is Lila's
//! own, smaller terminal view on top of them.

pub mod hc;
mod links;
mod sprites;
pub mod theme;
pub mod view;
pub mod vt;
mod win32_keys;

pub use hc::Core;
pub use view::TerminalView;

/// The libghostty-vt this build links, such as "1.0.0".
pub fn ghostty_version() -> String {
    // Never NULL, owned by the library for as long as it is loaded.
    unsafe { std::ffi::CStr::from_ptr(ghostty_sys::ghostty_version()) }.to_string_lossy().into_owned()
}
