use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub const fn from_hex(hex: u32) -> Self {
        Self { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8 }
    }

    /// 向 `other` 混合，`amount` 为 0 时不变，为 1 时等于 `other`。
    pub fn blend(self, other: Rgb, amount: f32) -> Rgb {
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
        Rgb::new(mix(self.r, other.r), mix(self.g, other.g), mix(self.b, other.b))
    }
}

/// 终端配色。程序用 OSC 序列改过的颜色优先于这里的默认值。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    /// 0-7 普通色，8-15 高亮色。
    pub ansi: [Rgb; 16],
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            foreground: Rgb::from_hex(0xd4d4d8),
            background: Rgb::from_hex(0x16161a),
            cursor: Rgb::from_hex(0xe4e4e7),
            ansi: [
                0x26262b, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
                0x565f89, 0xff899d, 0xb9f27c, 0xff9e64, 0x8db0ff, 0xc7a9ff, 0xa4daff, 0xffffff,
            ]
            .map(Rgb::from_hex),
        }
    }
}

impl Palette {
    pub(crate) fn resolve(&self, color: Color, overrides: &Colors) -> Rgb {
        match color {
            Color::Spec(rgb) => Rgb::new(rgb.r, rgb.g, rgb.b),
            Color::Indexed(index) => overrides[index as usize]
                .map(|rgb| Rgb::new(rgb.r, rgb.g, rgb.b))
                .unwrap_or_else(|| self.indexed(index)),
            Color::Named(name) => overrides[name]
                .map(|rgb| Rgb::new(rgb.r, rgb.g, rgb.b))
                .unwrap_or_else(|| self.named(name)),
        }
    }

    fn named(&self, name: NamedColor) -> Rgb {
        // 编号见 alacritty_terminal::term::color：0-15 ANSI，256 前景，257 背景，
        // 258 光标，259-266 暗色，267 高亮前景，268 暗前景。
        match name as usize {
            index @ 0..=15 => self.ansi[index],
            257 => self.background,
            258 => self.cursor,
            index @ 259..=266 => self.ansi[index - 259].blend(self.background, 0.4),
            268 => self.foreground.blend(self.background, 0.4),
            _ => self.foreground,
        }
    }

    /// xterm 的 256 色：16 个 ANSI 色、6x6x6 色块、24 级灰阶。
    fn indexed(&self, index: u8) -> Rgb {
        match index {
            0..=15 => self.ansi[index as usize],
            16..=231 => {
                let index = index - 16;
                let level = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
                Rgb::new(level(index / 36), level(index / 6 % 6), level(index % 6))
            }
            232..=255 => {
                let gray = 8 + (index - 232) * 10;
                Rgb::new(gray, gray, gray)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_colors_follow_xterm() {
        let palette = Palette::default();
        assert_eq!(palette.indexed(16), Rgb::new(0, 0, 0));
        assert_eq!(palette.indexed(196), Rgb::new(255, 0, 0));
        assert_eq!(palette.indexed(231), Rgb::new(255, 255, 255));
        assert_eq!(palette.indexed(232), Rgb::new(8, 8, 8));
        assert_eq!(palette.indexed(255), Rgb::new(238, 238, 238));
    }

    #[test]
    fn blend_endpoints() {
        let a = Rgb::new(0, 100, 200);
        let b = Rgb::new(100, 0, 0);
        assert_eq!(a.blend(b, 0.0), a);
        assert_eq!(a.blend(b, 1.0), b);
    }
}
