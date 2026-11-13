//! 界面配色与字体。终端内部的颜色在 nexsh_term::Palette 里。

use gpui::{Hsla, Rgba};

pub const WINDOW_BG: u32 = 0x1b1b20;
pub const SIDEBAR_BG: u32 = 0x202026;
pub const BORDER: u32 = 0x2e2e36;
pub const HOVER_BG: u32 = 0x2a2a32;
pub const INPUT_BG: u32 = 0x16161a;
pub const ACCENT: u32 = 0x7aa2f7;
pub const TEXT: u32 = 0xd4d4d8;
pub const TEXT_MUTED: u32 = 0x8b8b96;
pub const SUCCESS: u32 = 0x9ece6a;
pub const ERROR: u32 = 0xf7768e;
pub const NOTICE: u32 = 0xe0af68;

pub const FONT_SIZE: f32 = 14.0;
/// 行高相对字号的倍数。
pub const LINE_HEIGHT: f32 = 1.35;

#[cfg(target_os = "macos")]
pub const FONT_FAMILY: &str = "Menlo";
#[cfg(target_os = "windows")]
pub const FONT_FAMILY: &str = "Consolas";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const FONT_FAMILY: &str = "DejaVu Sans Mono";

/// 终端颜色转成 GPUI 颜色。
pub fn color(rgb: nexsh_term::Rgb) -> Hsla {
    Rgba {
        r: rgb.r as f32 / 255.0,
        g: rgb.g as f32 / 255.0,
        b: rgb.b as f32 / 255.0,
        a: 1.0,
    }
    .into()
}
