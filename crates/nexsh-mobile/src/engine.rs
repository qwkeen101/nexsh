//! 会话引擎：每个会话拥有一个终端模拟器和一条 SSH 连接，在后台把远端输出喂进终端。
//!
//! 桌面端的终端归视图所有、在界面线程上推进；手机上界面是另一种语言写的，隔着 JNI，
//! 所以这里反过来：会话自己在 IO 线程上推进，界面只在每帧来问一句"画面变了没有"。
//! 代数没变就什么都不返回，静止的终端不耗电。
//!
//! 断线重连也在这里：手机换基站、切 Wi-Fi、锁屏后被系统掐网都是常事，连接断了由会话自己
//! 按退避间隔重试，界面只是显示它的状态。只有连上过的会话才自动重连——第一次就连不上
//! 多半是地址填错了，反复重试没有意义。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use nexsh_core::{
    AuthMethod, ConnectOptions, Host, LatencyMeter, SessionCommand, SessionEvent, SshError,
    spawn_session,
};
use nexsh_term::{KeyMods, SelectionEnd, TermEvent, Terminal, bracketed_paste, encode_key};
use tokio::sync::mpsc;

use crate::wire::{self, Snapshot, State};

/// 连续这么多次延迟探测没有回应，就当连接已经死了。探测每 5 秒一次，
/// 所以大约 15 秒发现断线；只靠 SSH 保活要等 45 秒以上。
const MISSED_PROBES_LIMIT: u32 = 3;
/// 重连间隔的上限。
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

/// 断线时写进终端的内容：退出备用屏幕，关掉上一个 shell 里的程序打开的各种模式
/// （应用光标键、括号粘贴、鼠标上报、滚动区域），再留一行标记。重连得到的是一个新 shell，
/// 这些模式不清掉，方向键和粘贴就会按旧程序的约定发出去。光标位置在重设滚动区域前后保存和恢复。
const DISCONNECT_MARK: &str = "\x1b[?1049l\x1b[?1l\x1b>\x1b[?2004l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\
\x1b[?25h\x1b[0m\x1b7\x1b[r\x1b8\r\n\x1b[2m── 连接已断开 ──\x1b[0m\r\n";

/// 第 attempt 次重连（从 1 数起）之前等多久：1、2、4、8、16 秒，之后每次 30 秒。
fn retry_delay(attempt: u32) -> Duration {
    Duration::from_secs(1 << attempt.saturating_sub(1).min(5)).min(MAX_RETRY_DELAY)
}

/// 登录凭据。只留在内存里，重连时接着用；界面一侧负责加密保存。
#[derive(Clone, Default)]
pub struct Credentials {
    pub password: Option<String>,
    pub passphrase: Option<String>,
    /// 私钥内容。
    pub key: Option<String>,
}

/// 一次已经排好的重连：等 delay 之后，凭 ticket 来兑现。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Retry {
    ticket: u64,
    delay: Duration,
}

struct Inner {
    terminal: Terminal,
    state: State,
    title: String,
    message: String,
    latency: LatencyMeter,
    commands: Option<mpsc::UnboundedSender<SessionCommand>>,
    /// 第几次连接。重连后，上一次连接迟到的事件凭它被丢弃。
    epoch: u64,
    credentials: Credentials,
    /// 主机只允许私钥登录：这时认证失败和密码无关。
    key_only: bool,
    /// 正在进行第几次重连；0 表示不在重连。
    attempt: u32,
    /// 最近排出的那次重连的编号，以及它是否还在等待。提前重连或取消之后，
    /// 定时器到点时发现编号对不上或已不在等待，就什么都不做。
    ticket: u64,
    waiting: bool,
    missed_probes: u32,
}

impl Inner {
    fn send(&self, command: SessionCommand) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(command);
        }
    }

    /// 处理一个连接事件。返回 Some 表示连接断了，调用方要按它排一次重连。
    fn apply(&mut self, event: SessionEvent) -> Option<Retry> {
        match event {
            SessionEvent::HostKeyLearned(fingerprint) => {
                self.message = format!("首次连接这台主机，已记住它的密钥 {fingerprint}");
            }
            SessionEvent::Connected => {
                self.state = State::Connected;
                self.attempt = 0;
                self.missed_probes = 0;
            }
            SessionEvent::Output(bytes) => {
                self.terminal.feed(&bytes);
                for event in self.terminal.take_events() {
                    match event {
                        TermEvent::Title(title) => self.title = title,
                        TermEvent::ResetTitle => self.title.clear(),
                        TermEvent::PtyWrite(reply) => self.send(SessionCommand::Input(reply)),
                        TermEvent::Bell => {}
                    }
                }
            }
            SessionEvent::Latency(sample) => {
                self.missed_probes = if sample.is_some() { 0 } else { self.missed_probes + 1 };
                self.latency.record(sample);
                if self.state == State::Connected && self.missed_probes >= MISSED_PROBES_LIMIT {
                    return Some(self.lost());
                }
            }
            // 带退出码的是 shell 自己结束的（用户输入了 exit）；没有退出码是连接断了。
            SessionEvent::Exited(Some(code)) => {
                self.state = State::Exited;
                self.message = format!("会话已结束（退出码 {code}）");
            }
            SessionEvent::Exited(None) => {
                if self.state == State::Connected {
                    return Some(self.lost());
                }
                self.state = State::Exited;
                self.message = "连接已断开".to_string();
            }
            SessionEvent::Failed(error) => {
                let transient = matches!(
                    error,
                    SshError::Timeout(_) | SshError::Connect { .. } | SshError::Protocol(_)
                );
                if transient && (self.state == State::Connected || self.attempt > 0) {
                    return Some(self.lost());
                }
                self.fail(error);
            }
            SessionEvent::Closed => {
                if self.state == State::Connected {
                    return Some(self.lost());
                }
                if matches!(self.state, State::Connecting | State::Reconnecting) {
                    self.state = State::Exited;
                    self.message = "连接已断开".to_string();
                }
                self.commands = None;
            }
        }
        None
    }

    /// 连接断了（或者一次重连没成功）：丢掉它，排下一次重连。
    fn lost(&mut self) -> Retry {
        if self.state == State::Connected {
            self.terminal.feed(DISCONNECT_MARK.as_bytes());
            self.terminal.take_events();
            self.title.clear();
        }
        self.send(SessionCommand::Close);
        self.commands = None;
        // 这条连接之后的事件一律作废。
        self.epoch += 1;
        self.attempt += 1;
        self.ticket += 1;
        self.waiting = true;
        self.missed_probes = 0;
        self.state = State::Reconnecting;
        let delay = retry_delay(self.attempt);
        self.message = format!("连接已断开，{} 秒后第 {} 次重连", delay.as_secs(), self.attempt);
        Retry { ticket: self.ticket, delay }
    }

    /// 不再重连，停在"已断开"。
    fn settle(&mut self, state: State, message: String) {
        self.attempt = 0;
        self.waiting = false;
        self.state = state;
        self.message = message;
    }

    /// 缺密码、缺口令不算失败：界面据此弹出输入框，拿到后再连一次。
    /// 用过的密码或口令被拒绝时把它忘掉，免得下次重连又拿它去试。
    fn fail(&mut self, error: SshError) {
        let (state, message) = match error {
            SshError::PasswordRequired => (State::NeedsPassword, String::new()),
            SshError::PassphraseRequired(source) => {
                let message = match self.credentials.passphrase.take() {
                    Some(_) => "口令不正确".to_string(),
                    None => format!("{source}有口令保护"),
                };
                (State::NeedsPassphrase, message)
            }
            SshError::AuthFailed { .. } if !self.key_only && self.credentials.password.is_some() => {
                self.credentials.password = None;
                (State::NeedsPassword, "密码不正确".to_string())
            }
            other => (State::Failed, other.to_string()),
        };
        self.settle(state, message);
    }
}

pub struct Session {
    host: Host,
    known_hosts: PathBuf,
    inner: Mutex<Inner>,
    generation: AtomicU64,
}

impl Session {
    fn new(
        host: Host,
        known_hosts: PathBuf,
        cols: usize,
        rows: usize,
        credentials: Credentials,
    ) -> Self {
        let key_only = host.auth == AuthMethod::Key;
        Self {
            host,
            known_hosts,
            inner: Mutex::new(Inner {
                terminal: Terminal::new(cols, rows),
                state: State::Connecting,
                title: String::new(),
                message: String::new(),
                latency: LatencyMeter::default(),
                commands: None,
                epoch: 0,
                credentials,
                key_only,
                attempt: 0,
                ticket: 0,
                waiting: false,
                missed_probes: 0,
            }),
            // 从 1 开始：界面第一次拿 0 来问，一定能取到画面。
            generation: AtomicU64::new(1),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn touch(&self) {
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// 用户发起（或重新发起）连接。给出的密码、口令替换掉记着的，没给的沿用。
    pub fn connect(self: &Arc<Self>, password: Option<String>, passphrase: Option<String>) {
        let mut inner = self.lock();
        if password.is_some() {
            inner.credentials.password = password;
        }
        if passphrase.is_some() {
            inner.credentials.passphrase = passphrase;
        }
        inner.attempt = 0;
        self.start(inner);
    }

    /// 开一条新连接。旧连接随它的命令通道一起丢弃，它迟到的事件因为 epoch 变了而作废。
    fn start(self: &Arc<Self>, mut inner: MutexGuard<'_, Inner>) {
        inner.epoch += 1;
        inner.waiting = false;
        inner.missed_probes = 0;
        if inner.attempt > 0 {
            inner.state = State::Reconnecting;
            inner.message = format!("正在重新连接（第 {} 次）", inner.attempt);
        } else {
            inner.state = State::Connecting;
            inner.message.clear();
        }
        let (cols, rows) = inner.terminal.size();
        let mut options = ConnectOptions::new(self.host.clone(), cols as u32, rows as u32);
        options.password = inner.credentials.password.clone();
        options.key_passphrase = inner.credentials.passphrase.clone();
        options.identity = inner.credentials.key.clone();
        options.known_hosts_path = Some(self.known_hosts.clone());
        // 手机上的凭据由系统密钥库加密后存在应用自己的存储里，由界面一侧传进来。
        options.saved_secrets = None;
        let handle = spawn_session(options);
        inner.commands = Some(handle.commands);
        let epoch = inner.epoch;
        drop(inner);
        self.touch();
        nexsh_core::runtime().spawn(pump(self.clone(), epoch, handle.events));
    }

    /// 把事件记到会话上。外层的 None 表示这是旧连接的事件，已丢弃；
    /// 内层的 Some 是需要排的重连。和 [`Session::deliver`] 分开，是为了不碰网络就能测试状态变化。
    fn absorb(&self, epoch: u64, event: SessionEvent) -> Option<Option<Retry>> {
        let mut inner = self.lock();
        if inner.epoch != epoch {
            return None;
        }
        let retry = inner.apply(event);
        drop(inner);
        self.touch();
        Some(retry)
    }

    fn deliver(self: &Arc<Self>, epoch: u64, event: SessionEvent) -> bool {
        let Some(retry) = self.absorb(epoch, event) else { return false };
        if let Some(Retry { ticket, delay }) = retry {
            let session = self.clone();
            nexsh_core::runtime().spawn(async move {
                tokio::time::sleep(delay).await;
                let inner = session.lock();
                if inner.waiting && inner.ticket == ticket {
                    session.start(inner);
                }
            });
        }
        true
    }

    /// 不等退避时间到，马上重连。网络恢复、应用回到前台、用户点"立即重连"时调用；
    /// 会话不在等待重连时什么都不做。
    pub fn retry_now(self: &Arc<Self>) {
        let inner = self.lock();
        if inner.waiting {
            self.start(inner);
        }
    }

    /// 用户不想再重连了。
    pub fn stop_retry(&self) {
        let mut inner = self.lock();
        if inner.state != State::Reconnecting {
            return;
        }
        inner.epoch += 1;
        inner.send(SessionCommand::Close);
        inner.commands = None;
        inner.settle(State::Exited, "连接已断开".to_string());
        drop(inner);
        self.touch();
    }

    /// 原样写给远端的字节。打字时画面回到最新一行，选区取消。
    pub fn input(&self, bytes: Vec<u8>) {
        let mut inner = self.lock();
        inner.terminal.clear_selection();
        inner.terminal.scroll_to_bottom();
        inner.send(SessionCommand::Input(bytes));
        drop(inner);
        self.touch();
    }

    /// 功能键（"enter"、"up"、"f5" 等）。返回 false 表示这不是功能键，应当走 [`Session::text`]。
    pub fn key(&self, name: &str, mods: KeyMods) -> bool {
        let bytes = encode_key(name, mods, self.lock().terminal.app_cursor());
        match bytes {
            Some(bytes) => {
                self.input(bytes);
                true
            }
            None => false,
        }
    }

    /// 输入法提交的文字。按住了 Ctrl 或 Alt 且只有一个字符时按组合键编码（Ctrl+C 等）。
    pub fn text(&self, text: &str, mods: KeyMods) {
        let mut chars = text.chars();
        let single = matches!((chars.next(), chars.next()), (Some(_), None));
        if single && (mods.ctrl || mods.alt) && self.key(text, mods) {
            return;
        }
        self.input(text.as_bytes().to_vec());
    }

    pub fn paste(&self, text: &str) {
        let bracketed = self.lock().terminal.bracketed_paste();
        self.input(bracketed_paste(text, bracketed));
    }

    pub fn resize(&self, cols: usize, rows: usize) {
        let mut inner = self.lock();
        if inner.terminal.resize(cols, rows) {
            let (cols, rows) = inner.terminal.size();
            inner.send(SessionCommand::Resize { cols: cols as u32, rows: rows as u32 });
            drop(inner);
            self.touch();
        }
    }

    /// 正数向上翻看历史。
    pub fn scroll(&self, lines: i32) {
        self.lock().terminal.scroll(lines);
        self.touch();
    }

    /// 改动选区，然后让界面重画。
    fn select(&self, change: impl FnOnce(&mut Terminal)) {
        change(&mut self.lock().terminal);
        self.touch();
    }

    /// 选中可见区域里这个位置上的词。
    pub fn select_word(&self, row: usize, col: usize) {
        self.select(|terminal| terminal.select_word(row, col));
    }

    /// 把选区的一端拖到这个位置。
    pub fn move_selection(&self, end: SelectionEnd, row: usize, col: usize) {
        self.select(|terminal| terminal.move_selection(end, row, col));
    }

    pub fn select_all(&self) {
        self.select(Terminal::select_all);
    }

    pub fn clear_selection(&self) {
        self.select(Terminal::clear_selection);
    }

    pub fn selection_text(&self) -> Option<String> {
        self.lock().terminal.selection_text()
    }

    /// 画面自 `since` 那一代以来没变时返回 None。
    pub fn snapshot(&self, since: u64) -> Option<Vec<u8>> {
        let generation = self.generation.load(Ordering::Acquire);
        if generation == since {
            return None;
        }
        let inner = self.lock();
        let frame = inner.terminal.frame();
        let connected = inner.state == State::Connected;
        Some(wire::encode(&Snapshot {
            state: inner.state,
            generation,
            app_cursor: inner.terminal.app_cursor(),
            bracketed_paste: inner.terminal.bracketed_paste(),
            alt_screen: inner.terminal.alt_screen(),
            latency_ms: inner.latency.millis().filter(|_| connected),
            title: &inner.title,
            message: &inner.message,
            frame: &frame,
        }))
    }

    fn close(&self) {
        let mut inner = self.lock();
        // 之后迟到的事件一律作废，排着的重连也不再兑现。
        inner.epoch += 1;
        inner.waiting = false;
        inner.send(SessionCommand::Close);
        inner.commands = None;
    }
}

async fn pump(session: Arc<Session>, epoch: u64, mut events: mpsc::Receiver<SessionEvent>) {
    while let Some(event) = events.recv().await {
        if !session.deliver(epoch, event) {
            return;
        }
    }
}

/// 所有打开着的会话。界面用编号指代会话，编号从 1 开始、不复用。
pub struct Engine {
    known_hosts: PathBuf,
    sessions: Mutex<HashMap<u64, Arc<Session>>>,
    next_id: AtomicU64,
}

impl Engine {
    pub fn new(known_hosts: PathBuf) -> Self {
        Self { known_hosts, sessions: Mutex::new(HashMap::new()), next_id: AtomicU64::new(1) }
    }

    fn sessions(&self) -> MutexGuard<'_, HashMap<u64, Arc<Session>>> {
        self.sessions.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn open(
        &self,
        host: Host,
        cols: usize,
        rows: usize,
        credentials: Credentials,
    ) -> u64 {
        let session =
            Arc::new(Session::new(host, self.known_hosts.clone(), cols, rows, credentials));
        session.connect(None, None);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.sessions().insert(id, session);
        id
    }

    pub fn session(&self, id: u64) -> Option<Arc<Session>> {
        self.sessions().get(&id).cloned()
    }

    pub fn close(&self, id: u64) {
        if let Some(session) = self.sessions().remove(&id) {
            session.close();
        }
    }

    /// 网络可能刚恢复：等着重连的会话都马上试一次。
    pub fn retry_all(&self) {
        let sessions: Vec<Arc<Session>> = self.sessions().values().cloned().collect();
        for session in sessions {
            session.retry_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_with(credentials: Credentials) -> Session {
        let host = Host::parse_target("tester@example.com").unwrap();
        Session::new(host, PathBuf::from("known_hosts"), 20, 4, credentials)
    }

    fn session() -> Session {
        session_with(Credentials::default())
    }

    fn state(session: &Session) -> State {
        session.lock().state
    }

    /// 以当前的 epoch 送进一个事件，返回它要求排的重连。
    fn feed(session: &Session, event: SessionEvent) -> Option<Retry> {
        let epoch = session.lock().epoch;
        session.absorb(epoch, event).expect("当前连接的事件不该被丢弃")
    }

    fn refused() -> SessionEvent {
        SessionEvent::Failed(SshError::Connect { address: "a:22".into(), message: "拒绝".into() })
    }

    #[test]
    fn output_changes_the_picture_and_the_generation() {
        let session = session();
        let first = session.snapshot(0).expect("第一次总能取到画面");
        let generation = u64::from_le_bytes(first[4..12].try_into().unwrap());
        assert!(session.snapshot(generation).is_none());

        feed(&session, SessionEvent::Connected);
        feed(&session, SessionEvent::Output(b"hello".to_vec()));
        let next = session.snapshot(generation).expect("有输出后画面变了");
        assert_eq!(next[1], State::Connected as u8);
        assert!(next.len() > first.len());
    }

    #[test]
    fn selecting_redraws_and_typing_clears_it() {
        let session = session();
        feed(&session, SessionEvent::Output(b"hello world".to_vec()));
        let generation = session.generation.load(Ordering::Acquire);
        session.select_word(0, 7);
        assert_eq!(session.selection_text().as_deref(), Some("world"));
        let frame = session.snapshot(generation).expect("选区变了要重画");
        assert_eq!(frame[2] & 8, 8);

        session.input(b"x".to_vec());
        assert_eq!(session.selection_text(), None);
    }

    #[test]
    fn missing_credentials_ask_instead_of_failing() {
        let session = session();
        feed(&session, SessionEvent::Failed(SshError::PasswordRequired));
        assert_eq!(state(&session), State::NeedsPassword);

        // 没给过密码时认证失败就是失败；给过密码则是密码错了，忘掉它再问一次。
        let tried = || SessionEvent::Failed(SshError::AuthFailed { tried: vec!["密码".to_string()] });
        feed(&session, tried());
        assert_eq!(state(&session), State::Failed);
        session.lock().credentials.password = Some("wrong".to_string());
        feed(&session, tried());
        assert_eq!(state(&session), State::NeedsPassword);
        assert!(session.lock().credentials.password.is_none());
    }

    #[test]
    fn a_rejected_passphrase_is_forgotten() {
        let session = session_with(Credentials {
            passphrase: Some("wrong".to_string()),
            key: Some("key".to_string()),
            ..Default::default()
        });
        let required =
            || SessionEvent::Failed(SshError::PassphraseRequired(nexsh_core::KeySource::Imported));
        feed(&session, required());
        assert_eq!(state(&session), State::NeedsPassphrase);
        assert_eq!(session.lock().message, "口令不正确");
        assert!(session.lock().credentials.passphrase.is_none());
        feed(&session, required());
        assert_eq!(session.lock().message, "导入的私钥有口令保护");
    }

    #[test]
    fn key_only_hosts_do_not_blame_the_password() {
        let mut host = Host::parse_target("tester@example.com").unwrap();
        host.auth = AuthMethod::Key;
        let credentials = Credentials { password: Some("unused".to_string()), ..Default::default() };
        let session = Session::new(host, PathBuf::from("known_hosts"), 20, 4, credentials);
        feed(&session, SessionEvent::Failed(SshError::AuthFailed { tried: vec![] }));
        assert_eq!(state(&session), State::Failed);
    }

    #[test]
    fn events_from_an_earlier_connection_are_dropped() {
        let session = session();
        session.lock().epoch = 3;
        assert!(session.absorb(2, SessionEvent::Connected).is_none());
        assert_eq!(state(&session), State::Connecting);
        assert!(session.absorb(3, SessionEvent::Connected).is_some());
        assert_eq!(state(&session), State::Connected);
    }

    #[test]
    fn a_first_connection_that_fails_is_not_retried() {
        let session = session();
        assert_eq!(feed(&session, refused()), None);
        assert_eq!(state(&session), State::Failed);
        assert_eq!(feed(&session, SessionEvent::Closed), None);
        assert_eq!(state(&session), State::Failed);
    }

    #[test]
    fn leaving_the_shell_is_not_a_dropped_connection() {
        let session = session();
        feed(&session, SessionEvent::Connected);
        assert_eq!(feed(&session, SessionEvent::Exited(Some(0))), None);
        assert_eq!(feed(&session, SessionEvent::Closed), None);
        assert_eq!(state(&session), State::Exited);
    }

    #[test]
    fn a_dropped_connection_is_retried_with_backoff() {
        let session = session();
        feed(&session, SessionEvent::Connected);
        let first = feed(&session, SessionEvent::Exited(None)).expect("断线后要重连");
        assert_eq!(first.delay, Duration::from_secs(1));
        assert_eq!(state(&session), State::Reconnecting);
        // 断掉的那条连接随后发来的 Closed 已经不算数。
        assert!(session.absorb(0, SessionEvent::Closed).is_none());

        // 重连没连上：间隔翻倍，编号换新，上一次排的定时器因此失效。
        let second = feed(&session, refused()).expect("没连上要接着试");
        assert_eq!(second.delay, Duration::from_secs(2));
        assert_ne!(second.ticket, first.ticket);
        assert_eq!(session.lock().attempt, 2);

        // 连上之后从头计数。
        session.lock().waiting = false;
        feed(&session, SessionEvent::Connected);
        assert_eq!(session.lock().attempt, 0);
        let again = feed(&session, SessionEvent::Closed).expect("没有 Exited 的关闭也是断线");
        assert_eq!(again.delay, Duration::from_secs(1));
    }

    #[test]
    fn retry_delays_double_up_to_the_cap() {
        let seconds: Vec<u64> = (1..=8).map(|attempt| retry_delay(attempt).as_secs()).collect();
        assert_eq!(seconds, [1, 2, 4, 8, 16, 30, 30, 30]);
        assert_eq!(retry_delay(u32::MAX), MAX_RETRY_DELAY);
    }

    #[test]
    fn unanswered_probes_count_as_a_dropped_connection() {
        let session = session();
        feed(&session, SessionEvent::Connected);
        assert_eq!(feed(&session, SessionEvent::Latency(None)), None);
        assert_eq!(feed(&session, SessionEvent::Latency(None)), None);
        // 中间有一次回应，重新数。
        assert_eq!(feed(&session, SessionEvent::Latency(Some(Duration::from_millis(20)))), None);
        assert_eq!(feed(&session, SessionEvent::Latency(None)), None);
        assert_eq!(feed(&session, SessionEvent::Latency(None)), None);
        assert!(feed(&session, SessionEvent::Latency(None)).is_some());
        assert_eq!(state(&session), State::Reconnecting);
    }

    #[test]
    fn a_drop_resets_the_modes_left_by_the_old_shell() {
        let session = session();
        feed(&session, SessionEvent::Connected);
        // 全屏程序常开的三样：备用屏幕、应用光标键、括号粘贴。
        feed(&session, SessionEvent::Output(b"\x1b[?1049h\x1b[?1h\x1b[?2004h".to_vec()));
        {
            let inner = session.lock();
            assert!(inner.terminal.alt_screen() && inner.terminal.app_cursor());
            assert!(inner.terminal.bracketed_paste());
        }
        feed(&session, SessionEvent::Exited(None));
        let inner = session.lock();
        assert!(!inner.terminal.alt_screen() && !inner.terminal.app_cursor());
        assert!(!inner.terminal.bracketed_paste());
        assert!(inner.terminal.text().iter().any(|line| line.starts_with("──")));
    }

    #[test]
    fn stopping_ends_the_retries() {
        let session = session();
        feed(&session, SessionEvent::Connected);
        feed(&session, SessionEvent::Exited(None));
        session.stop_retry();
        assert_eq!(state(&session), State::Exited);
        assert!(!session.lock().waiting);
        // 不在重连时调用没有效果。
        session.stop_retry();
        assert_eq!(state(&session), State::Exited);
    }

    #[test]
    fn a_changed_host_key_stops_the_retries() {
        let session = session();
        feed(&session, SessionEvent::Connected);
        feed(&session, SessionEvent::Exited(None));
        session.lock().waiting = false;
        let changed = SshError::HostKeyChanged { host: "example.com".into(), line: 1 };
        assert_eq!(feed(&session, SessionEvent::Failed(changed)), None);
        assert_eq!(state(&session), State::Failed);
        assert_eq!(session.lock().attempt, 0);
    }
}
