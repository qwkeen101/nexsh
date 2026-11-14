//! 预测回显：往返延迟高的时候，把刚敲下的字先画在光标处，不等服务器回显。
//!
//! 链路慢（几百毫秒）时，每个字都要等一个往返才出现，打字像在泥里走。大多数时候回显的结果
//! 是确定的：在 shell 提示符的行尾敲一个字母，它就出现在光标处。这里把这个结果先画出来，
//! 服务器的回显到了再对账——对得上就悄悄换成真的，对不上就撤掉。
//!
//! 预测只叠加在给界面的那一帧上，从不写进终端本身：终端里永远只有服务器发来的内容，
//! 预测错了撤掉叠加层即可，不需要回滚任何状态。
//!
//! 三条保守的规则保证它不帮倒忙：
//!
//! - **先证实，再显示。** 每一行开头的预测先藏着，等其中第一个字被服务器回显证实了才显示。
//!   密码提示不回显，预测永远得不到证实，密码也就永远不会出现在屏幕上；vim 这类程序的回显
//!   和预测对不上，同样不显示。
//! - **只预测行尾的简单输入。** 可打印的 ASCII 字符，以及删掉自己刚敲的字的退格。回车、
//!   方向键、Tab、组合键、粘贴都不预测，并且在它们的结果回来之前暂停预测——那时光标会去哪里
//!   只有服务器知道。
//! - **拿不准就放弃。** 光标右边有内容、快到行尾要折行、备用屏幕、对账不上、迟迟没有回显，
//!   都直接清空，回到等服务器回显的老办法。

use std::time::{Duration, Instant};

use crate::{Cell, Cursor, Frame, Terminal};

/// 往返延迟低于这个值时不预测：回显本来就够快，预测只会带来偶尔的闪动。
const MIN_ROUND_TRIP: Duration = Duration::from_millis(60);
/// 同时挂着的预测上限。超过说明服务器很久没回显了，再猜下去没有意义。
const MAX_PENDING: usize = 64;
/// 预测最少等这么久才算"服务器不会回显了"。
const MIN_PATIENCE: Duration = Duration::from_secs(2);
/// 不知道延迟时，不可预测的输入之后暂停多久。
const DEFAULT_PAUSE: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edit {
    Put(char),
    Erase,
}

#[derive(Debug, Default)]
pub struct Predictor {
    /// 正在输入的这一行在屏幕上的行号，和这一段输入开始的列。
    row: usize,
    origin: usize,
    /// 从 origin 起、已经被服务器回显证实的字符。退格只能删到这里为止——
    /// 再往左是提示符或者别的不是我们敲的东西。
    confirmed: Vec<char>,
    /// 已经发出、还没有被回显证实的编辑，按输入顺序，各自带着发出的时刻。
    pending: Vec<(Edit, Instant)>,
    /// 这一段输入里已经有预测被证实过，可以显示了。
    trusted: bool,
    /// 不可预测的输入还在路上，到这个时刻之前不预测。
    paused_until: Option<Instant>,
    /// 一个预测最多等多久。
    patience: Duration,
}

impl Predictor {
    /// 前 count 个编辑都生效之后，这一段输入应该是什么样。
    fn outcome(&self, count: usize) -> Vec<char> {
        let mut line = self.confirmed.clone();
        for (edit, _) in &self.pending[..count] {
            match edit {
                Edit::Put(ch) => line.push(*ch),
                Edit::Erase => {
                    line.pop();
                }
            }
        }
        line
    }

    fn forget(&mut self) {
        self.pending.clear();
        self.confirmed.clear();
        self.trusted = false;
    }

    /// 回到初始状态。连接断开、重连、终端改了尺寸时调用。
    pub fn reset(&mut self) {
        self.forget();
        self.paused_until = None;
    }

    /// 发出了不预测的输入（回车、方向键、组合键、粘贴等）。它的结果回来之前光标在哪里说不准，
    /// 所以放弃现有的预测，并暂停一个往返多一点的时间。
    pub fn disturbed(&mut self, round_trip: Option<Duration>, now: Instant) {
        self.forget();
        let pause = round_trip.map(|rtt| rtt.mul_f32(1.5)).unwrap_or(DEFAULT_PAUSE);
        self.paused_until = Some(now + pause);
    }

    /// 现在能不能在行尾接着预测 extra 个字符。
    fn ready(&mut self, extra: usize, terminal: &Terminal, round_trip: Option<Duration>, now: Instant) -> bool {
        let Some(round_trip) = round_trip.filter(|rtt| *rtt >= MIN_ROUND_TRIP) else { return false };
        if self.paused_until.is_some_and(|until| now < until) {
            return false;
        }
        self.paused_until = None;
        if terminal.alt_screen() || terminal.is_scrolled() {
            return false;
        }
        let cursor = terminal.cursor_cell();
        let (cols, _) = terminal.size();
        if self.pending.is_empty() {
            // 没有在途的预测时，服务器的光标应当正好在已证实的那一段后面；不在，说明这是新的一行
            // （或者画面被别的输出动过），从这里重新开始，并且重新等待证实。
            if cursor.row != self.row || cursor.col != self.origin + self.confirmed.len() {
                self.forget();
                self.row = cursor.row;
                self.origin = cursor.col;
            }
            if looks_secret(terminal, cursor.row) {
                return false;
            }
        } else if cursor.row != self.row {
            return false;
        }
        // 光标右边有内容时，行编辑器会把它们右移，结果不只是"多一个字"。
        if (cursor.col..cols).any(|col| terminal.char_at(cursor.row, col) != ' ') {
            return false;
        }
        // 留出最后一列：写满一行之后的折行各家终端和 shell 的处理不一样。
        let end = self.origin + self.outcome(self.pending.len()).len();
        if end + extra + 1 >= cols || self.pending.len() + extra > MAX_PENDING {
            return false;
        }
        self.patience = (round_trip * 4).max(MIN_PATIENCE);
        true
    }

    /// 用户输入了一段文字（已经发给服务器）。
    pub fn typed(&mut self, text: &str, terminal: &Terminal, round_trip: Option<Duration>, now: Instant) {
        let plain = !text.is_empty() && text.chars().all(|ch| ch == ' ' || ch.is_ascii_graphic());
        if plain && self.ready(text.len(), terminal, round_trip, now) {
            self.pending.extend(text.chars().map(|ch| (Edit::Put(ch), now)));
        } else {
            self.disturbed(round_trip, now);
        }
    }

    /// 用户按了退格（已经发给服务器）。只预测删掉这一段里自己敲的字。
    pub fn erased(&mut self, terminal: &Terminal, round_trip: Option<Duration>, now: Instant) {
        let ours = self.ready(0, terminal, round_trip, now) && !self.outcome(self.pending.len()).is_empty();
        if ours {
            self.pending.push((Edit::Erase, now));
        } else {
            self.disturbed(round_trip, now);
        }
    }

    /// 屏幕上从 origin 起是不是正好是 line，后面跟着光标和一个空位。
    fn shows(&self, terminal: &Terminal, line: &[char], cursor: Cursor) -> bool {
        cursor.col == self.origin + line.len()
            && line.iter().enumerate().all(|(index, ch)| terminal.char_at(self.row, self.origin + index) == *ch)
            && terminal.char_at(self.row, cursor.col) == ' '
    }

    /// 服务器来了新的输出（已经喂进终端）。看它证实了多少预测；和任何一步都对不上就全部放弃。
    ///
    /// 回显可能分几次到，所以从"全部生效"往回找第一个和屏幕对得上的中间状态。
    pub fn reconcile(&mut self, terminal: &Terminal, now: Instant) {
        if self.pending.is_empty() {
            return;
        }
        let cursor = terminal.cursor_cell();
        let reached = (!terminal.alt_screen() && cursor.row == self.row)
            .then(|| {
                (0..=self.pending.len()).rev().find_map(|count| {
                    let line = self.outcome(count);
                    self.shows(terminal, &line, cursor).then_some((count, line))
                })
            })
            .flatten();
        match reached {
            Some((0, _)) => self.expire(now),
            Some((count, line)) => {
                self.confirmed = line;
                self.pending.drain(..count);
                self.trusted = true;
            }
            None => self.forget(),
        }
    }

    /// 等得太久的预测不会再被证实了（密码提示、不回显的程序），放弃。
    fn expire(&mut self, now: Instant) {
        if self.pending.first().is_some_and(|(_, sent)| now.duration_since(*sent) > self.patience) {
            self.forget();
        }
    }

    /// 把还没证实的预测画到这一帧上：预测的字符用淡一些的颜色，光标移到预测的位置。
    pub fn overlay(&mut self, frame: &mut Frame, terminal: &Terminal, now: Instant) {
        self.expire(now);
        if self.pending.is_empty() || !self.trusted || terminal.is_scrolled() {
            return;
        }
        if terminal.cursor_cell().row != self.row {
            return;
        }
        let line = self.outcome(self.pending.len());
        // 已证实的部分里没被退格动到的保持原样（服务器可能给它们上了颜色），只改后面的。
        let kept = self.confirmed.iter().zip(&line).take_while(|(shown, wanted)| shown == wanted).count();
        let from = self.origin + kept;
        let until = self.origin + self.confirmed.len().max(line.len());
        let row = self.row;
        frame.cells.retain(|cell| !(cell.row == row && cell.col >= from && cell.col < until));
        let fg = frame.foreground.blend(frame.background, 0.35);
        for (index, ch) in line.iter().enumerate().skip(kept) {
            if *ch == ' ' {
                continue;
            }
            frame.cells.push(Cell {
                row,
                col: self.origin + index,
                ch: *ch,
                fg,
                bg: frame.background,
                bold: false,
                italic: false,
                underline: false,
                strikeout: false,
                wide: false,
                selected: false,
            });
        }
        if frame.cursor.is_some() {
            frame.cursor = Some(Cursor { row, col: self.origin + line.len() });
        }
    }
}

/// 这一行像不像在问密码。不回显的提示本来就得不到证实，这里是多一道保险：
/// 有的程序用星号回显密码，第一个字碰巧也是星号时不该被当成证实。
fn looks_secret(terminal: &Terminal, row: usize) -> bool {
    let (cols, _) = terminal.size();
    let line: String = (0..cols).map(|col| terminal.char_at(row, col)).collect::<String>().to_lowercase();
    ["password", "passphrase", "passcode", "密码", "口令"].iter().any(|word| line.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLOW: Option<Duration> = Some(Duration::from_millis(400));

    /// 叠加预测之后某一行的文字（去掉行尾空白）和光标所在的列。
    fn view(predictor: &mut Predictor, terminal: &Terminal, now: Instant) -> (String, usize) {
        let mut frame = terminal.frame();
        predictor.overlay(&mut frame, terminal, now);
        let cursor = frame.cursor.expect("光标可见");
        let mut line = vec![' '; frame.cols];
        for cell in frame.cells.iter().filter(|cell| cell.row == cursor.row) {
            line[cell.col] = cell.ch;
        }
        (line.into_iter().collect::<String>().trim_end().to_string(), cursor.col)
    }

    fn prompt() -> (Terminal, Predictor, Instant) {
        let mut terminal = Terminal::new(40, 5);
        terminal.feed(b"$ ");
        (terminal, Predictor::default(), Instant::now())
    }

    /// 敲一个字并让服务器回显它：之后这一行的预测就可以显示了。
    fn trusted_prompt() -> (Terminal, Predictor, Instant) {
        let (mut terminal, mut predictor, now) = prompt();
        predictor.typed("l", &terminal, SLOW, now);
        terminal.feed(b"l");
        predictor.reconcile(&terminal, now);
        (terminal, predictor, now)
    }

    #[test]
    fn hides_predictions_until_the_first_echo_confirms_them() {
        let (mut terminal, mut predictor, now) = prompt();
        predictor.typed("l", &terminal, SLOW, now);
        predictor.typed("s", &terminal, SLOW, now);
        assert_eq!(view(&mut predictor, &terminal, now), ("$".to_string(), 2));

        // 第一个字的回显到了：第二个字不用再等。
        terminal.feed(b"l");
        predictor.reconcile(&terminal, now);
        assert_eq!(view(&mut predictor, &terminal, now), ("$ ls".to_string(), 4));

        // 之后敲的字立刻出现。
        predictor.typed(" -la", &terminal, SLOW, now);
        assert_eq!(view(&mut predictor, &terminal, now), ("$ ls -la".to_string(), 8));

        // 回显全部到齐，叠加层空了，画面就是服务器的画面。
        terminal.feed(b"s -la");
        predictor.reconcile(&terminal, now);
        assert!(predictor.pending.is_empty());
        assert_eq!(view(&mut predictor, &terminal, now), ("$ ls -la".to_string(), 8));
    }

    #[test]
    fn never_shows_what_is_typed_at_a_prompt_that_does_not_echo() {
        let mut terminal = Terminal::new(40, 5);
        terminal.feed(b"Enter PIN: ");
        let mut predictor = Predictor::default();
        let now = Instant::now();
        predictor.typed("1234", &terminal, SLOW, now);
        assert_eq!(view(&mut predictor, &terminal, now), ("Enter PIN:".to_string(), 11));
        // 等够了时间还没有回显：放弃，不留着占位。
        let later = now + Duration::from_secs(10);
        assert_eq!(view(&mut predictor, &terminal, later), ("Enter PIN:".to_string(), 11));
        assert!(predictor.pending.is_empty());
    }

    #[test]
    fn does_not_predict_at_a_password_prompt() {
        let mut terminal = Terminal::new(40, 5);
        terminal.feed(b"[sudo] password for me: ");
        let mut predictor = Predictor::default();
        let now = Instant::now();
        predictor.typed("s", &terminal, SLOW, now);
        assert!(predictor.pending.is_empty());
    }

    #[test]
    fn drops_predictions_the_server_contradicts() {
        let (mut terminal, mut predictor, now) = prompt();
        predictor.typed("ab", &terminal, SLOW, now);
        terminal.feed(b"*");
        predictor.reconcile(&terminal, now);
        assert!(predictor.pending.is_empty());
        assert!(!predictor.trusted);
        assert_eq!(view(&mut predictor, &terminal, now), ("$ *".to_string(), 3));
    }

    #[test]
    fn predicts_erasing_what_was_just_typed() {
        let (mut terminal, mut predictor, now) = trusted_prompt();
        predictor.typed("x", &terminal, SLOW, now);
        predictor.erased(&terminal, SLOW, now);
        assert_eq!(view(&mut predictor, &terminal, now), ("$ l".to_string(), 3));
        // 再退一格，删的是已经回显过的那个字。
        predictor.erased(&terminal, SLOW, now);
        assert_eq!(view(&mut predictor, &terminal, now), ("$".to_string(), 2));

        // 服务器照做：先回显 x，再退两格并清到行尾。
        terminal.feed(b"x\x08\x1b[K\x08\x1b[K");
        predictor.reconcile(&terminal, now);
        assert!(predictor.pending.is_empty());
        assert!(predictor.confirmed.is_empty());
        assert_eq!(view(&mut predictor, &terminal, now), ("$".to_string(), 2));
    }

    #[test]
    fn does_not_erase_past_its_own_input() {
        let (terminal, mut predictor, now) = prompt();
        // 提示符不是我们敲的，退格删不删得掉只有服务器知道。
        predictor.erased(&terminal, SLOW, now);
        assert!(predictor.pending.is_empty());
        assert!(predictor.paused_until.is_some());
    }

    #[test]
    fn pauses_after_input_it_cannot_predict() {
        let (terminal, mut predictor, now) = trusted_prompt();
        // 回车：命令的输出和新的提示符还在路上。
        predictor.disturbed(SLOW, now);
        predictor.typed("x", &terminal, SLOW, now + Duration::from_millis(100));
        assert!(predictor.pending.is_empty());
        // 暂停过后重新开始，但要重新等证实。
        predictor.typed("y", &terminal, SLOW, now + Duration::from_secs(5));
        assert_eq!(predictor.pending.len(), 1);
        assert!(!predictor.trusted);
    }

    #[test]
    fn leaves_fast_links_and_full_screen_programs_alone() {
        let (mut terminal, mut predictor, now) = prompt();
        predictor.typed("a", &terminal, Some(Duration::from_millis(20)), now);
        assert!(predictor.pending.is_empty());

        predictor.reset();
        terminal.feed(b"\x1b[?1049h");
        predictor.typed("a", &terminal, SLOW, now);
        assert!(predictor.pending.is_empty());
    }

    #[test]
    fn does_not_predict_in_the_middle_of_a_line() {
        let (mut terminal, mut predictor, now) = prompt();
        // 光标左移到已有文字中间。
        terminal.feed(b"hello\x1b[3D");
        predictor.typed("a", &terminal, SLOW, now);
        assert!(predictor.pending.is_empty());
    }

    #[test]
    fn stops_before_the_line_would_wrap() {
        let mut terminal = Terminal::new(10, 3);
        terminal.feed(b"$ ");
        let mut predictor = Predictor::default();
        let now = Instant::now();
        predictor.typed("abcdef", &terminal, SLOW, now);
        assert_eq!(predictor.pending.len(), 6);
        predictor.typed("gh", &terminal, SLOW, now);
        assert!(predictor.pending.is_empty());
    }
}
