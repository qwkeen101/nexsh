//! 会话引擎：每个会话拥有一个终端模拟器和一条 SSH 连接，在后台把远端输出喂进终端。
//!
//! 桌面端的终端归视图所有、在界面线程上推进；手机上界面是另一种语言写的，隔着 JNI，
//! 所以这里反过来：会话自己在 IO 线程上推进，界面只在每帧来问一句"画面变了没有"。
//! 代数没变就什么都不返回，静止的终端不耗电。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use nexsh_core::{
    ConnectOptions, Host, LatencyMeter, SessionCommand, SessionEvent, SshError, spawn_session,
};
use nexsh_term::{KeyMods, TermEvent, Terminal, bracketed_paste, encode_key};
use tokio::sync::mpsc;

use crate::wire::{self, Snapshot, State};

struct Inner {
    terminal: Terminal,
    state: State,
    title: String,
    message: String,
    latency: LatencyMeter,
    commands: Option<mpsc::UnboundedSender<SessionCommand>>,
    /// 第几次连接。重连后，上一次连接迟到的事件凭它被丢弃。
    epoch: u64,
    /// 这次连接带了密码：认证失败时要重新问密码，而不是直接报错。
    offered_password: bool,
}

impl Inner {
    fn send(&self, command: SessionCommand) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(command);
        }
    }

    fn apply(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::HostKeyLearned(fingerprint) => {
                self.message = format!("首次连接这台主机，已记住它的密钥 {fingerprint}");
            }
            SessionEvent::Connected => self.state = State::Connected,
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
            SessionEvent::Latency(sample) => self.latency.record(sample),
            SessionEvent::Exited(code) => {
                self.state = State::Exited;
                self.message = match code {
                    Some(code) => format!("会话已结束（退出码 {code}）"),
                    None => "连接已断开".to_string(),
                };
            }
            SessionEvent::Failed(error) => self.fail(error),
            SessionEvent::Closed => {
                if matches!(self.state, State::Connecting | State::Connected) {
                    self.state = State::Exited;
                    self.message = "连接已断开".to_string();
                }
                self.commands = None;
            }
        }
    }

    /// 缺密码、缺口令不算失败：界面据此弹出输入框，拿到后再连一次。
    fn fail(&mut self, error: SshError) {
        (self.state, self.message) = match error {
            SshError::PasswordRequired => (State::NeedsPassword, String::new()),
            SshError::PassphraseRequired(path) => {
                (State::NeedsPassphrase, format!("私钥 {} 有口令保护", path.display()))
            }
            SshError::AuthFailed { .. } if self.offered_password => {
                (State::NeedsPassword, "密码不正确".to_string())
            }
            other => (State::Failed, other.to_string()),
        };
    }
}

pub struct Session {
    host: Host,
    known_hosts: PathBuf,
    inner: Mutex<Inner>,
    generation: AtomicU64,
}

impl Session {
    fn new(host: Host, known_hosts: PathBuf, cols: usize, rows: usize) -> Self {
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
                offered_password: false,
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

    /// 发起（或重新发起）连接。旧连接随它的命令通道一起丢弃。
    pub fn connect(self: &Arc<Self>, password: Option<String>, passphrase: Option<String>) {
        let (epoch, events) = {
            let mut inner = self.lock();
            inner.epoch += 1;
            inner.state = State::Connecting;
            inner.message.clear();
            inner.offered_password = password.is_some();
            let (cols, rows) = inner.terminal.size();
            let mut options = ConnectOptions::new(self.host.clone(), cols as u32, rows as u32);
            options.password = password;
            options.key_passphrase = passphrase;
            options.known_hosts_path = Some(self.known_hosts.clone());
            // 手机上的密码由系统密钥库加密后存在应用自己的存储里，由界面一侧传进来。
            options.saved_secrets = None;
            let handle = spawn_session(options);
            inner.commands = Some(handle.commands);
            (inner.epoch, handle.events)
        };
        self.touch();
        nexsh_core::runtime().spawn(pump(self.clone(), epoch, events));
    }

    fn deliver(&self, epoch: u64, event: SessionEvent) -> bool {
        let mut inner = self.lock();
        if inner.epoch != epoch {
            return false;
        }
        inner.apply(event);
        drop(inner);
        self.touch();
        true
    }

    /// 原样写给远端的字节。打字时画面回到最新一行。
    pub fn input(&self, bytes: Vec<u8>) {
        let mut inner = self.lock();
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
        // 之后迟到的事件一律作废。
        inner.epoch += 1;
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
        password: Option<String>,
        passphrase: Option<String>,
    ) -> u64 {
        let session = Arc::new(Session::new(host, self.known_hosts.clone(), cols, rows));
        session.connect(password, passphrase);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let host = Host::parse_target("tester@example.com").unwrap();
        Session::new(host, PathBuf::from("known_hosts"), 20, 4)
    }

    fn state(session: &Session) -> State {
        session.lock().state
    }

    #[test]
    fn output_changes_the_picture_and_the_generation() {
        let session = session();
        let first = session.snapshot(0).expect("第一次总能取到画面");
        let generation = u64::from_le_bytes(first[4..12].try_into().unwrap());
        assert!(session.snapshot(generation).is_none());

        assert!(session.deliver(0, SessionEvent::Connected));
        assert!(session.deliver(0, SessionEvent::Output(b"hello".to_vec())));
        let next = session.snapshot(generation).expect("有输出后画面变了");
        assert_eq!(next[1], State::Connected as u8);
        assert!(next.len() > first.len());
    }

    #[test]
    fn missing_credentials_ask_instead_of_failing() {
        let session = session();
        session.deliver(0, SessionEvent::Failed(SshError::PasswordRequired));
        assert_eq!(state(&session), State::NeedsPassword);

        // 没给过密码时认证失败就是失败；给过密码则是密码错了，再问一次。
        let tried = || SshError::AuthFailed { tried: vec!["密码".to_string()] };
        session.deliver(0, SessionEvent::Failed(tried()));
        assert_eq!(state(&session), State::Failed);
        session.lock().offered_password = true;
        session.deliver(0, SessionEvent::Failed(tried()));
        assert_eq!(state(&session), State::NeedsPassword);
    }

    #[test]
    fn events_from_an_earlier_connection_are_dropped() {
        let session = session();
        session.lock().epoch = 3;
        assert!(!session.deliver(2, SessionEvent::Connected));
        assert_eq!(state(&session), State::Connecting);
        assert!(session.deliver(3, SessionEvent::Connected));
        assert_eq!(state(&session), State::Connected);
    }

    #[test]
    fn closing_the_channel_ends_a_live_session() {
        let session = session();
        session.deliver(0, SessionEvent::Connected);
        session.deliver(0, SessionEvent::Closed);
        assert_eq!(state(&session), State::Exited);
    }
}
