//! 终端模拟：把远端输出解析成可绘制的字符网格，把按键编码成终端序列。
//! VT 解析由 alacritty_terminal 完成，这里对外只暴露与界面框架无关的类型。

mod input;
mod palette;

pub use input::{KeyMods, bracketed_paste, encode_key};
pub use palette::{Palette, Rgb};

use std::sync::{Arc, Mutex};

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{self, CursorShape};

/// 终端主动发出的、需要界面处理的事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TermEvent {
    Title(String),
    ResetTitle,
    /// 终端对查询序列的应答，必须原样写回远端。
    PtyWrite(Vec<u8>),
    Bell,
}

#[derive(Clone)]
struct EventProxy(Arc<Mutex<Vec<TermEvent>>>);

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        let event = match event {
            Event::Title(title) => TermEvent::Title(title),
            Event::ResetTitle => TermEvent::ResetTitle,
            Event::PtyWrite(text) => TermEvent::PtyWrite(text.into_bytes()),
            Event::Bell => TermEvent::Bell,
            _ => return,
        };
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(event);
    }
}

struct GridSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// 一个需要绘制的单元格。空白且背景为默认色的单元格不会出现在 [`Frame`] 里。
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub row: usize,
    pub col: usize,
    pub ch: char,
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    /// 占两列的宽字符（中日韩文字等）。
    pub wide: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
}

/// 当前可见区域的快照。
#[derive(Clone, Debug)]
pub struct Frame {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
    /// 光标隐藏或滚出可见区域时为 None。
    pub cursor: Option<Cursor>,
    pub background: Rgb,
    pub foreground: Rgb,
}

pub struct Terminal {
    term: Term<EventProxy>,
    parser: ansi::Processor,
    events: Arc<Mutex<Vec<TermEvent>>>,
    palette: Palette,
    cols: usize,
    rows: usize,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        let (cols, rows) = clamp_size(cols, rows);
        let events = Arc::new(Mutex::new(Vec::new()));
        let term = Term::new(Config::default(), &GridSize { cols, rows }, EventProxy(events.clone()));
        Self { term, parser: ansi::Processor::default(), events, palette: Palette::default(), cols, rows }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    /// 喂入远端输出。
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// 返回尺寸是否真的变了。
    pub fn resize(&mut self, cols: usize, rows: usize) -> bool {
        let (cols, rows) = clamp_size(cols, rows);
        if (cols, rows) == (self.cols, self.rows) {
            return false;
        }
        self.cols = cols;
        self.rows = rows;
        self.term.resize(GridSize { cols, rows });
        true
    }

    /// 取走自上次调用以来积累的事件。
    pub fn take_events(&mut self) -> Vec<TermEvent> {
        std::mem::take(&mut *self.events.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// 终端的默认背景色。
    pub fn frame_background(&self) -> Rgb {
        self.palette.background
    }

    /// 应用是否要求方向键使用应用模式序列（vim、less 等会开启）。
    pub fn app_cursor(&self) -> bool {
        self.term.mode().contains(TermMode::APP_CURSOR)
    }

    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    pub fn alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// 正数向上翻看历史，负数向下。
    pub fn scroll(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    pub fn frame(&self) -> Frame {
        let content = self.term.renderable_content();
        let colors = content.colors;
        let offset = content.display_offset as i32;
        let background = self.palette.background;
        let mut cells = Vec::new();

        for indexed in content.display_iter {
            let cell = indexed.cell;
            let flags = cell.flags;
            if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let row = indexed.point.line.0 + offset;
            if row < 0 || row as usize >= self.rows {
                continue;
            }

            let mut fg = self.palette.resolve(cell.fg, colors);
            let mut bg = self.palette.resolve(cell.bg, colors);
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if flags.contains(Flags::DIM) {
                fg = fg.blend(bg, 0.4);
            }
            if flags.contains(Flags::HIDDEN) {
                fg = bg;
            }
            if cell.c == ' ' && bg == background {
                continue;
            }

            cells.push(Cell {
                row: row as usize,
                col: indexed.point.column.0,
                ch: cell.c,
                fg,
                bg,
                bold: flags.contains(Flags::BOLD),
                italic: flags.contains(Flags::ITALIC),
                underline: flags.intersects(Flags::ALL_UNDERLINES),
                strikeout: flags.contains(Flags::STRIKEOUT),
                wide: flags.contains(Flags::WIDE_CHAR),
            });
        }

        let cursor_row = content.cursor.point.line.0 + offset;
        let cursor = (content.cursor.shape != CursorShape::Hidden
            && cursor_row >= 0
            && (cursor_row as usize) < self.rows)
            .then(|| Cursor { row: cursor_row as usize, col: content.cursor.point.column.0 });

        Frame {
            cols: self.cols,
            rows: self.rows,
            cells,
            cursor,
            background,
            foreground: self.palette.foreground,
        }
    }

    /// 可见区域的纯文本，每行去掉行尾空白。供测试和调试用。
    pub fn text(&self) -> Vec<String> {
        let frame = self.frame();
        let mut lines = vec![vec![' '; frame.cols]; frame.rows];
        for cell in &frame.cells {
            lines[cell.row][cell.col] = cell.ch;
        }
        lines.into_iter().map(|line| line.into_iter().collect::<String>().trim_end().to_string()).collect()
    }
}

fn clamp_size(cols: usize, rows: usize) -> (usize, usize) {
    (cols.max(2), rows.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_plain_text() {
        let mut term = Terminal::new(20, 4);
        term.feed(b"hello\r\nworld");
        let text = term.text();
        assert_eq!(text[0], "hello");
        assert_eq!(text[1], "world");
        assert_eq!(term.frame().cursor, Some(Cursor { row: 1, col: 5 }));
    }

    #[test]
    fn applies_sgr_colors_and_attributes() {
        let mut term = Terminal::new(20, 2);
        term.feed(b"\x1b[1;31mR\x1b[0m\x1b[48;2;1;2;3mB");
        let frame = term.frame();
        let red = frame.cells.iter().find(|c| c.ch == 'R').unwrap();
        assert!(red.bold);
        assert_eq!(red.fg, Palette::default().ansi[1]);
        let blue = frame.cells.iter().find(|c| c.ch == 'B').unwrap();
        assert_eq!(blue.bg, Rgb::new(1, 2, 3));
        assert!(!blue.bold);
    }

    #[test]
    fn wide_characters_occupy_one_cell_entry() {
        let mut term = Terminal::new(20, 2);
        term.feed("中a".as_bytes());
        let frame = term.frame();
        assert_eq!(frame.cells.len(), 2);
        assert!(frame.cells[0].wide);
        assert_eq!((frame.cells[0].col, frame.cells[1].col), (0, 2));
    }

    #[test]
    fn answers_device_status_query() {
        let mut term = Terminal::new(20, 2);
        term.feed(b"ab\x1b[6n");
        assert_eq!(term.take_events(), vec![TermEvent::PtyWrite(b"\x1b[1;3R".to_vec())]);
        assert!(term.take_events().is_empty());
    }

    #[test]
    fn tracks_title_and_modes() {
        let mut term = Terminal::new(20, 2);
        term.feed(b"\x1b]0;my title\x07\x1b[?1h\x1b[?2004h");
        assert_eq!(term.take_events(), vec![TermEvent::Title("my title".into())]);
        assert!(term.app_cursor());
        assert!(term.bracketed_paste());
    }

    #[test]
    fn scrollback_is_reachable() {
        let mut term = Terminal::new(10, 2);
        term.feed(b"one\r\ntwo\r\nthree");
        assert_eq!(term.text(), ["two", "three"]);
        term.scroll(1);
        assert_eq!(term.text(), ["one", "two"]);
        term.scroll_to_bottom();
        assert_eq!(term.text(), ["two", "three"]);
    }

    #[test]
    fn resize_reports_changes() {
        let mut term = Terminal::new(10, 2);
        assert!(!term.resize(10, 2));
        assert!(term.resize(30, 5));
        assert_eq!(term.size(), (30, 5));
        assert!(!term.resize(0, 0) || term.size() == (2, 1));
    }
}
