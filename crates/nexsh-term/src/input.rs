//! 把按键翻译成发给远端的字节序列（xterm 约定）。
//!
//! 按键名采用 GPUI 的写法："enter"、"up"、"pageup"、"f5"、"a" 等。
//! 普通可打印字符不在这里处理，它们走文本输入（含输入法）路径。

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyMods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl KeyMods {
    fn any(self) -> bool {
        self.ctrl || self.alt || self.shift
    }

    /// xterm 的修饰键参数：1 + shift(1) + alt(2) + ctrl(4)。
    fn param(self) -> u8 {
        1 + self.shift as u8 + 2 * self.alt as u8 + 4 * self.ctrl as u8
    }
}

/// 返回 None 表示这个按键应当作为普通文本输入处理。
pub fn encode_key(key: &str, mods: KeyMods, app_cursor: bool) -> Option<Vec<u8>> {
    let esc_prefixed = |bytes: &[u8]| {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        if mods.alt {
            out.push(0x1b);
        }
        out.extend_from_slice(bytes);
        out
    };

    match key {
        "enter" => return Some(esc_prefixed(b"\r")),
        "escape" => return Some(esc_prefixed(b"\x1b")),
        "tab" if mods.shift => return Some(b"\x1b[Z".to_vec()),
        "tab" => return Some(esc_prefixed(b"\t")),
        "backspace" if mods.ctrl => return Some(esc_prefixed(b"\x08")),
        "backspace" => return Some(esc_prefixed(b"\x7f")),
        "space" if mods.ctrl => return Some(esc_prefixed(b"\0")),
        _ => {}
    }

    // 方向键和 Home/End：ESC [ X，应用模式下 ESC O X，带修饰键时 ESC [ 1 ; m X。
    let letter = match key {
        "up" => Some(b'A'),
        "down" => Some(b'B'),
        "right" => Some(b'C'),
        "left" => Some(b'D'),
        "home" => Some(b'H'),
        "end" => Some(b'F'),
        _ => None,
    };
    if let Some(letter) = letter {
        return Some(if mods.any() {
            format!("\x1b[1;{}{}", mods.param(), letter as char).into_bytes()
        } else if app_cursor {
            vec![0x1b, b'O', letter]
        } else {
            vec![0x1b, b'[', letter]
        });
    }

    // F1-F4 用 SS3 序列，带修饰键时改用 CSI 形式。
    let ss3 = match key {
        "f1" => Some(b'P'),
        "f2" => Some(b'Q'),
        "f3" => Some(b'R'),
        "f4" => Some(b'S'),
        _ => None,
    };
    if let Some(letter) = ss3 {
        return Some(if mods.any() {
            format!("\x1b[1;{}{}", mods.param(), letter as char).into_bytes()
        } else {
            vec![0x1b, b'O', letter]
        });
    }

    // 其余功能键：ESC [ n ~，带修饰键时 ESC [ n ; m ~。
    let tilde = match key {
        "insert" => Some(2),
        "delete" => Some(3),
        "pageup" => Some(5),
        "pagedown" => Some(6),
        "f5" => Some(15),
        "f6" => Some(17),
        "f7" => Some(18),
        "f8" => Some(19),
        "f9" => Some(20),
        "f10" => Some(21),
        "f11" => Some(23),
        "f12" => Some(24),
        _ => None,
    };
    if let Some(code) = tilde {
        return Some(if mods.any() {
            format!("\x1b[{code};{}~", mods.param()).into_bytes()
        } else {
            format!("\x1b[{code}~").into_bytes()
        });
    }

    let mut chars = key.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else { return None };

    if mods.ctrl {
        let byte = match ch.to_ascii_lowercase() {
            c @ 'a'..='z' => c as u8 - b'a' + 1,
            '@' | '2' => 0x00,
            '[' | '3' => 0x1b,
            '\\' | '4' => 0x1c,
            ']' | '5' => 0x1d,
            '^' | '6' => 0x1e,
            '_' | '-' | '7' | '/' => 0x1f,
            '?' | '8' => 0x7f,
            _ => return None,
        };
        return Some(esc_prefixed(&[byte]));
    }

    if mods.alt {
        // Alt 当作 Meta：发送 ESC 前缀。
        let ch = if mods.shift { ch.to_ascii_uppercase() } else { ch };
        let mut out = vec![0x1b];
        out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
        return Some(out);
    }

    None
}

/// 粘贴文本：换行统一成回车；远端开启了括号粘贴模式时加上包裹序列，
/// 这样 shell 不会把粘贴内容里的换行当成立即执行。
pub fn bracketed_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if !bracketed {
        return normalized.into_bytes();
    }
    // 去掉文本里的结束标记，防止粘贴内容提前跳出括号粘贴。
    let safe = normalized.replace("\x1b[201~", "");
    let mut out = b"\x1b[200~".to_vec();
    out.extend_from_slice(safe.as_bytes());
    out.extend_from_slice(b"\x1b[201~");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: KeyMods = KeyMods { ctrl: false, alt: false, shift: false };
    const CTRL: KeyMods = KeyMods { ctrl: true, alt: false, shift: false };
    const ALT: KeyMods = KeyMods { ctrl: false, alt: true, shift: false };
    const SHIFT: KeyMods = KeyMods { ctrl: false, alt: false, shift: true };

    fn enc(key: &str, mods: KeyMods) -> Option<Vec<u8>> {
        encode_key(key, mods, false)
    }

    #[test]
    fn basic_keys() {
        assert_eq!(enc("enter", NONE).unwrap(), b"\r");
        assert_eq!(enc("backspace", NONE).unwrap(), b"\x7f");
        assert_eq!(enc("tab", NONE).unwrap(), b"\t");
        assert_eq!(enc("tab", SHIFT).unwrap(), b"\x1b[Z");
        assert_eq!(enc("escape", NONE).unwrap(), b"\x1b");
    }

    #[test]
    fn arrows_respect_application_mode() {
        assert_eq!(enc("up", NONE).unwrap(), b"\x1b[A");
        assert_eq!(encode_key("up", NONE, true).unwrap(), b"\x1bOA");
        // 带修饰键时不受应用模式影响。
        assert_eq!(encode_key("left", CTRL, true).unwrap(), b"\x1b[1;5D");
        assert_eq!(enc("right", KeyMods { ctrl: true, alt: false, shift: true }).unwrap(), b"\x1b[1;6C");
    }

    #[test]
    fn function_and_navigation_keys() {
        assert_eq!(enc("f1", NONE).unwrap(), b"\x1bOP");
        assert_eq!(enc("f5", NONE).unwrap(), b"\x1b[15~");
        assert_eq!(enc("f12", NONE).unwrap(), b"\x1b[24~");
        assert_eq!(enc("delete", NONE).unwrap(), b"\x1b[3~");
        assert_eq!(enc("pageup", SHIFT).unwrap(), b"\x1b[5;2~");
        assert_eq!(enc("home", NONE).unwrap(), b"\x1b[H");
    }

    #[test]
    fn control_and_alt_combinations() {
        assert_eq!(enc("c", CTRL).unwrap(), [0x03]);
        assert_eq!(enc("z", CTRL).unwrap(), [0x1a]);
        assert_eq!(enc("[", CTRL).unwrap(), [0x1b]);
        assert_eq!(enc("space", CTRL).unwrap(), [0x00]);
        assert_eq!(enc("b", ALT).unwrap(), b"\x1bb");
        assert_eq!(enc("c", KeyMods { ctrl: true, alt: true, shift: false }).unwrap(), [0x1b, 0x03]);
    }

    #[test]
    fn printable_keys_fall_through_to_text_input() {
        assert_eq!(enc("a", NONE), None);
        assert_eq!(enc("a", SHIFT), None);
        assert_eq!(enc("space", NONE), None);
        assert_eq!(enc("中", NONE), None);
    }

    #[test]
    fn paste_normalizes_newlines_and_brackets() {
        assert_eq!(bracketed_paste("a\r\nb\nc", false), b"a\rb\rc");
        assert_eq!(bracketed_paste("ls", true), b"\x1b[200~ls\x1b[201~");
        assert_eq!(bracketed_paste("x\x1b[201~rm", true), b"\x1b[200~xrm\x1b[201~");
    }
}
