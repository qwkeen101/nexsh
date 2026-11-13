//! 画面快照的二进制格式。界面每帧取一次，所以不用 JSON：一屏几千个单元格，
//! 定长记录直接按偏移读，Kotlin 一侧不产生任何临时对象。
//!
//! 所有整数小端。布局：
//!
//! | 偏移 | 类型 | 含义 |
//! |---|---|---|
//! | 0 | u8 | 版本，目前是 1 |
//! | 1 | u8 | 状态，见 [`State`] |
//! | 2 | u8 | 模式位：1 应用光标键，2 括号粘贴，4 备用屏幕 |
//! | 3 | u8 | 保留 |
//! | 4 | u64 | 代数，画面每变一次加一 |
//! | 12 | u16, u16 | 列数、行数 |
//! | 16 | i16, i16 | 光标的行、列；光标不可见时都是 -1 |
//! | 20 | u32, u32 | 默认背景色、默认前景色，0x00RRGGBB |
//! | 28 | i32 | 延迟毫秒；没有测到时为 -1 |
//! | 32 | u16 + 字节 | 标题（UTF-8） |
//! | … | u16 + 字节 | 提示信息（UTF-8） |
//! | … | u32 | 单元格数 |
//! | … | 17 字节 × N | 行 u16、列 u16、字符 u32、前景 u32、背景 u32、属性 u8 |
//!
//! 属性位：1 粗体，2 斜体，4 下划线，8 删除线，16 宽字符。

use nexsh_term::{Frame, Rgb};

pub const VERSION: u8 = 1;
pub const CELL_SIZE: usize = 17;

/// 会话所处的阶段。数值是协议的一部分，只能追加。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Connecting = 0,
    Connected = 1,
    NeedsPassword = 2,
    NeedsPassphrase = 3,
    Exited = 4,
    Failed = 5,
    /// 连接断了，正在等待重连或重连中；message 里是进度。
    Reconnecting = 6,
}

pub struct Snapshot<'a> {
    pub state: State,
    pub generation: u64,
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    pub alt_screen: bool,
    pub latency_ms: Option<u64>,
    pub title: &'a str,
    pub message: &'a str,
    pub frame: &'a Frame,
}

fn rgb(color: Rgb) -> u32 {
    (color.r as u32) << 16 | (color.g as u32) << 8 | color.b as u32
}

/// 超长的文字截到 u16 能表示的长度，且不切断一个 UTF-8 字符。
fn put_text(out: &mut Vec<u8>, text: &str) {
    let mut end = text.len().min(u16::MAX as usize);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    out.extend_from_slice(&(end as u16).to_le_bytes());
    out.extend_from_slice(&text.as_bytes()[..end]);
}

pub fn encode(snapshot: &Snapshot) -> Vec<u8> {
    let frame = snapshot.frame;
    let capacity =
        48 + snapshot.title.len() + snapshot.message.len() + frame.cells.len() * CELL_SIZE;
    let mut out = Vec::with_capacity(capacity);
    let modes = snapshot.app_cursor as u8
        | (snapshot.bracketed_paste as u8) << 1
        | (snapshot.alt_screen as u8) << 2;
    out.extend_from_slice(&[VERSION, snapshot.state as u8, modes, 0]);
    out.extend_from_slice(&snapshot.generation.to_le_bytes());
    out.extend_from_slice(&(frame.cols as u16).to_le_bytes());
    out.extend_from_slice(&(frame.rows as u16).to_le_bytes());
    let (cursor_row, cursor_col) = match frame.cursor {
        Some(cursor) => (cursor.row as i16, cursor.col as i16),
        None => (-1, -1),
    };
    out.extend_from_slice(&cursor_row.to_le_bytes());
    out.extend_from_slice(&cursor_col.to_le_bytes());
    out.extend_from_slice(&rgb(frame.background).to_le_bytes());
    out.extend_from_slice(&rgb(frame.foreground).to_le_bytes());
    let latency = snapshot.latency_ms.map(|ms| ms.min(i32::MAX as u64) as i32).unwrap_or(-1);
    out.extend_from_slice(&latency.to_le_bytes());
    put_text(&mut out, snapshot.title);
    put_text(&mut out, snapshot.message);
    out.extend_from_slice(&(frame.cells.len() as u32).to_le_bytes());
    for cell in &frame.cells {
        out.extend_from_slice(&(cell.row as u16).to_le_bytes());
        out.extend_from_slice(&(cell.col as u16).to_le_bytes());
        out.extend_from_slice(&(cell.ch as u32).to_le_bytes());
        out.extend_from_slice(&rgb(cell.fg).to_le_bytes());
        out.extend_from_slice(&rgb(cell.bg).to_le_bytes());
        let attrs = cell.bold as u8
            | (cell.italic as u8) << 1
            | (cell.underline as u8) << 2
            | (cell.strikeout as u8) << 3
            | (cell.wide as u8) << 4;
        out.push(attrs);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexsh_term::Terminal;

    #[test]
    fn layout_matches_the_documented_offsets() {
        let mut terminal = Terminal::new(20, 4);
        terminal.feed("hi".as_bytes());
        let frame = terminal.frame();
        let bytes = encode(&Snapshot {
            state: State::Connected,
            generation: 7,
            app_cursor: false,
            bracketed_paste: true,
            alt_screen: false,
            latency_ms: Some(42),
            title: "标题",
            message: "",
            frame: &frame,
        });
        assert_eq!(bytes[0], VERSION);
        assert_eq!(bytes[1], State::Connected as u8);
        assert_eq!(bytes[2], 2);
        assert_eq!(u64::from_le_bytes(bytes[4..12].try_into().unwrap()), 7);
        assert_eq!(u16::from_le_bytes(bytes[12..14].try_into().unwrap()), 20);
        assert_eq!(u16::from_le_bytes(bytes[14..16].try_into().unwrap()), 4);
        assert_eq!(i32::from_le_bytes(bytes[28..32].try_into().unwrap()), 42);
        let title_len = u16::from_le_bytes(bytes[32..34].try_into().unwrap()) as usize;
        assert_eq!(&bytes[34..34 + title_len], "标题".as_bytes());
        let message_at = 34 + title_len;
        assert_eq!(u16::from_le_bytes(bytes[message_at..message_at + 2].try_into().unwrap()), 0);
        let count_at = message_at + 2;
        let count = u32::from_le_bytes(bytes[count_at..count_at + 4].try_into().unwrap()) as usize;
        assert_eq!(count, frame.cells.len());
        assert_eq!(bytes.len(), count_at + 4 + count * CELL_SIZE);
        // 第一个单元格是第 0 行第 0 列的 h。
        let first = count_at + 4;
        assert_eq!(u32::from_le_bytes(bytes[first + 4..first + 8].try_into().unwrap()), 'h' as u32);
    }

    #[test]
    fn long_text_is_cut_on_a_character_boundary() {
        let mut out = Vec::new();
        let text = "中".repeat(30_000);
        put_text(&mut out, &text);
        let len = u16::from_le_bytes([out[0], out[1]]) as usize;
        assert_eq!(len % 3, 0);
        assert!(std::str::from_utf8(&out[2..2 + len]).is_ok());
    }
}
