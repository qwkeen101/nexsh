//! 界面配色与字体。终端内部的颜色在 nexsh_term::Palette 里。

use gpui::{Hsla, Rgba};

// 背景从深到浅分层：内容区（和终端同色）→ 侧栏与各条栏 → 浮起的对话框和菜单
// → 悬停 → 选中。每层只比下一层亮一点，靠明度区分层次而不是靠线条。
pub const WINDOW_BG: u32 = 0x16161a;
pub const SIDEBAR_BG: u32 = 0x1c1c21;
pub const SURFACE: u32 = 0x232329;
pub const BORDER: u32 = 0x2c2c33;
pub const HOVER_BG: u32 = 0x2a2a31;
pub const SELECTED_BG: u32 = 0x34343c;
pub const INPUT_BG: u32 = 0x131316;
pub const ACCENT: u32 = 0x6c9cf5;
pub const TEXT: u32 = 0xe4e4e8;
pub const TEXT_MUTED: u32 = 0x9a9aa4;
/// 比 TEXT_MUTED 更淡：计数、页脚这类可有可无的信息。
pub const TEXT_FAINT: u32 = 0x6b6b76;
pub const SUCCESS: u32 = 0x9ece6a;
pub const ERROR: u32 = 0xf7768e;
pub const NOTICE: u32 = 0xe0af68;

/// 圆角按元素大小嵌套：行和按钮、输入框、菜单、对话框。
pub const RADIUS_ITEM: f32 = 6.0;
pub const RADIUS_CONTROL: f32 = 7.0;
pub const RADIUS_MENU: f32 = 10.0;
pub const RADIUS_DIALOG: f32 = 12.0;
/// 侧栏标题栏和标签栏同高，两条底边连成一线。
pub const BAR_HEIGHT: f32 = 38.0;

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
