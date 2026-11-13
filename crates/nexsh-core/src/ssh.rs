//! SSH 会话：连接、主机密钥校验、认证、带 PTY 的交互式 shell。
//!
//! 会话跑在 [`crate::runtime`] 上，通过两个通道和调用方通信，调用方可以是任何
//! 执行器（界面线程不需要 tokio）。事件通道有容量上限：界面来不及消费时，
//! 这里会停止读取 SSH 通道，压力一路传回服务器，内存不会无限增长。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::keys::known_hosts::{check_known_hosts_path, learn_known_hosts_path};
use russh::keys::{self, HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect, client};
use tokio::sync::mpsc;

use crate::host::Host;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
/// 未被消费的事件上限，超过后对服务器施加背压。
const EVENT_QUEUE_CAPACITY: usize = 256;
/// 服务器通常在 6 次失败后断开（MaxAuthTries），给密钥文件和密码留出余量。
const MAX_AGENT_KEYS: usize = 4;
const TERM: &str = "xterm-256color";

#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("连接 {0} 超时")]
    Timeout(String),
    #[error("无法连接 {address}: {message}")]
    Connect { address: String, message: String },
    #[error(
        "{host} 的主机密钥与 known_hosts 第 {line} 行记录的不一致，已拒绝连接。\
         如果确认服务器重装过，先运行 ssh-keygen -R {host}"
    )]
    HostKeyChanged { host: String, line: usize },
    #[error("{host} 不在 known_hosts 中（指纹 {fingerprint}），当前策略拒绝未知主机")]
    HostKeyUnknown { host: String, fingerprint: String },
    #[error("{0} 使用主机证书，暂不支持校验")]
    HostCertificateUnsupported(String),
    #[error("无法读写 known_hosts: {0}")]
    KnownHosts(String),
    #[error("私钥 {} 有口令保护，需要输入口令", .0.display())]
    PassphraseRequired(PathBuf),
    #[error("密钥认证未通过，需要输入密码")]
    PasswordRequired,
    #[error("认证失败，已尝试: {}", .tried.join("、"))]
    AuthFailed { tried: Vec<String> },
    #[error("SSH 协议错误: {0}")]
    Protocol(#[from] russh::Error),
}

/// 遇到 known_hosts 里没有的主机时怎么办。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HostKeyPolicy {
    /// 接受并写入 known_hosts（相当于 OpenSSH 的 accept-new）。
    #[default]
    AcceptNew,
    /// 拒绝。
    Strict,
}

#[derive(Clone, Debug)]
pub struct ConnectOptions {
    pub host: Host,
    pub cols: u32,
    pub rows: u32,
    pub password: Option<String>,
    /// 私钥口令。空字符串表示跳过有口令保护的私钥。
    pub key_passphrase: Option<String>,
    pub host_key_policy: HostKeyPolicy,
    /// 为 None 时使用 `~/.ssh/known_hosts`。
    pub known_hosts_path: Option<PathBuf>,
}

impl ConnectOptions {
    pub fn new(host: Host, cols: u32, rows: u32) -> Self {
        Self {
            host,
            cols,
            rows,
            password: None,
            key_passphrase: None,
            host_key_policy: HostKeyPolicy::default(),
            known_hosts_path: None,
        }
    }
}

#[derive(Debug)]
pub enum SessionCommand {
    /// 写入远端 PTY 的字节。
    Input(Vec<u8>),
    Resize { cols: u32, rows: u32 },
    Close,
}

#[derive(Debug)]
pub enum SessionEvent {
    /// 首次连接该主机，密钥已写入 known_hosts；携带指纹供界面展示。
    HostKeyLearned(String),
    /// 认证通过，shell 已启动。
    Connected,
    /// 远端输出。
    Output(Vec<u8>),
    /// 远端 shell 结束，携带退出码（服务器没报告时为 None）。
    Exited(Option<u32>),
    Failed(SshError),
    /// 最后一个事件，之后通道关闭。
    Closed,
}

pub struct SessionHandle {
    pub commands: mpsc::UnboundedSender<SessionCommand>,
    pub events: mpsc::Receiver<SessionEvent>,
}

/// 在后台运行时上启动一个会话，立即返回。
pub fn spawn_session(options: ConnectOptions) -> SessionHandle {
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::channel(EVENT_QUEUE_CAPACITY);
    crate::runtime().spawn(async move {
        if let Err(error) = run_session(options, command_rx, &event_tx).await {
            let _ = event_tx.send(SessionEvent::Failed(error)).await;
        }
        let _ = event_tx.send(SessionEvent::Closed).await;
    });
    SessionHandle { commands: command_tx, events: event_rx }
}

/// 握手期间 handler 无法直接返回带上下文的错误，先记在这里。
#[derive(Default)]
struct HostKeyReport {
    learned: Option<String>,
    rejection: Option<SshError>,
}

struct ClientHandler {
    host: String,
    port: u16,
    policy: HostKeyPolicy,
    known_hosts: Option<PathBuf>,
    report: Arc<Mutex<HostKeyReport>>,
}

impl ClientHandler {
    fn verify(&self, key: &PublicKey) -> Result<Option<String>, SshError> {
        let path = self.known_hosts.as_ref().ok_or_else(|| {
            SshError::KnownHosts("找不到用户主目录".to_string())
        })?;
        match check_known_hosts_path(&self.host, self.port, key, path) {
            Ok(true) => Ok(None),
            Ok(false) => {
                let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
                match self.policy {
                    HostKeyPolicy::Strict => Err(SshError::HostKeyUnknown {
                        host: self.host.clone(),
                        fingerprint,
                    }),
                    HostKeyPolicy::AcceptNew => {
                        learn_known_hosts_path(&self.host, self.port, key, path)
                            .map_err(|e| SshError::KnownHosts(e.to_string()))?;
                        Ok(Some(fingerprint))
                    }
                }
            }
            Err(keys::Error::KeyChanged { line }) => {
                Err(SshError::HostKeyChanged { host: self.host.clone(), line })
            }
            Err(e) => Err(SshError::KnownHosts(e.to_string())),
        }
    }
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let outcome = match server_public_key {
            PublicKeyOrCertificate::PublicKey { key, .. } => self.verify(key),
            PublicKeyOrCertificate::Certificate(_) => {
                Err(SshError::HostCertificateUnsupported(self.host.clone()))
            }
        };
        let mut report = self.report.lock().unwrap_or_else(|e| e.into_inner());
        match outcome {
            Ok(learned) => {
                report.learned = learned;
                Ok(true)
            }
            Err(rejection) => {
                report.rejection = Some(rejection);
                Ok(false)
            }
        }
    }
}

async fn run_session(
    options: ConnectOptions,
    mut commands: mpsc::UnboundedReceiver<SessionCommand>,
    events: &mpsc::Sender<SessionEvent>,
) -> Result<(), SshError> {
    let host = &options.host;
    let address = format!("{}:{}", host.hostname, host.port);

    let config = Arc::new(client::Config {
        keepalive_interval: Some(KEEPALIVE_INTERVAL),
        keepalive_max: 3,
        // 关闭 Nagle：交互式终端里每个按键都应立刻发出。
        nodelay: true,
        ..Default::default()
    });
    let report = Arc::new(Mutex::new(HostKeyReport::default()));
    let handler = ClientHandler {
        host: host.hostname.clone(),
        port: host.port,
        policy: options.host_key_policy,
        known_hosts: options
            .known_hosts_path
            .clone()
            .or_else(|| std::env::home_dir().map(|home| home.join(".ssh").join("known_hosts"))),
        report: report.clone(),
    };

    let connecting = client::connect(config, (host.hostname.as_str(), host.port), handler);
    let connected = tokio::time::timeout(CONNECT_TIMEOUT, connecting).await;
    let (learned, rejection) = {
        let mut report = report.lock().unwrap_or_else(|e| e.into_inner());
        (report.learned.take(), report.rejection.take())
    };
    let mut handle = match connected {
        Err(_) => return Err(SshError::Timeout(address)),
        Ok(Err(error)) => {
            return Err(rejection
                .unwrap_or(SshError::Connect { address, message: error.to_string() }));
        }
        Ok(Ok(handle)) => handle,
    };
    if let Some(fingerprint) = learned {
        let _ = events.send(SessionEvent::HostKeyLearned(fingerprint)).await;
    }

    authenticate(&mut handle, &options).await?;

    let mut channel = handle.channel_open_session().await?;
    channel.request_pty(false, TERM, options.cols, options.rows, 0, 0, &[]).await?;
    channel.request_shell(true).await?;
    if events.send(SessionEvent::Connected).await.is_err() {
        return Ok(());
    }

    let mut exit_status = None;
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(SessionCommand::Input(bytes)) => channel.data_bytes(bytes).await?,
                Some(SessionCommand::Resize { cols, rows }) => {
                    channel.window_change(cols, rows, 0, 0).await?
                }
                Some(SessionCommand::Close) | None => break,
            },
            message = channel.wait() => match message {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    if events.send(SessionEvent::Output(data.to_vec())).await.is_err() {
                        break;
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status: code }) => exit_status = Some(code),
                Some(ChannelMsg::Close) | None => {
                    let _ = events.send(SessionEvent::Exited(exit_status)).await;
                    break;
                }
                Some(_) => {}
            },
        }
    }

    let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
    Ok(())
}

/// 依次尝试：ssh-agent、私钥文件、密码。
async fn authenticate(
    handle: &mut client::Handle<ClientHandler>,
    options: &ConnectOptions,
) -> Result<(), SshError> {
    let user = options.host.user.as_str();
    let rsa_hash = handle.best_supported_rsa_hash().await?.flatten();
    let mut tried = Vec::new();

    if try_agent(handle, user, rsa_hash, &mut tried).await {
        return Ok(());
    }

    let mut encrypted_key = None;
    for path in identity_candidates(&options.host) {
        // 先不带口令读：没加密的私钥不受口令影响，加密的再用口令重试。
        let loaded = match keys::load_secret_key(&path, None) {
            Err(keys::Error::KeyIsEncrypted) => match options.key_passphrase.as_deref() {
                None => {
                    encrypted_key.get_or_insert(path);
                    continue;
                }
                // 空口令表示用户选择跳过加密的私钥。
                Some("") => continue,
                Some(passphrase) => match keys::load_secret_key(&path, Some(passphrase)) {
                    Ok(key) => Ok(key),
                    // 口令不对：让调用方重新询问，而不是悄悄改用密码。
                    Err(_) => return Err(SshError::PassphraseRequired(path)),
                },
            },
            other => other,
        };
        match loaded {
            Ok(key) => {
                let key = PrivateKeyWithHashAlg::new(Arc::new(key), rsa_hash);
                if handle.authenticate_publickey(user, key).await?.success() {
                    return Ok(());
                }
                tried.push(format!("私钥 {}", path.display()));
            }
            Err(error) => tried.push(format!("私钥 {}（{error}）", path.display())),
        }
    }

    if let Some(password) = &options.password {
        if handle.authenticate_password(user, password.as_str()).await?.success() {
            return Ok(());
        }
        tried.push("密码".to_string());
    }

    // 还有没用上的凭据时，告诉调用方该向用户要什么。
    if let Some(path) = encrypted_key {
        return Err(SshError::PassphraseRequired(path));
    }
    if options.password.is_none() {
        return Err(SshError::PasswordRequired);
    }
    Err(SshError::AuthFailed { tried })
}

/// 主机指定了私钥就只用它，否则按 OpenSSH 的默认顺序找。
fn identity_candidates(host: &Host) -> Vec<PathBuf> {
    if let Some(path) = &host.identity_file {
        return vec![path.clone()];
    }
    let Some(home) = std::env::home_dir() else { return Vec::new() };
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .into_iter()
        .map(|name| home.join(".ssh").join(name))
        .filter(|path| path.is_file())
        .collect()
}

#[cfg(unix)]
async fn try_agent(
    handle: &mut client::Handle<ClientHandler>,
    user: &str,
    rsa_hash: Option<HashAlg>,
    tried: &mut Vec<String>,
) -> bool {
    use russh::keys::agent::AgentIdentity;
    use russh::keys::agent::client::AgentClient;

    let Ok(mut agent) = AgentClient::connect_env().await else { return false };
    let Ok(identities) = agent.request_identities().await else { return false };
    for identity in identities.into_iter().take(MAX_AGENT_KEYS) {
        let AgentIdentity::PublicKey { key, comment } = identity else { continue };
        let hash = if key.algorithm().is_rsa() { rsa_hash } else { None };
        match handle.authenticate_publickey_with(user, key, hash, &mut agent).await {
            Ok(result) if result.success() => return true,
            _ => tried.push(format!("ssh-agent 密钥 {comment}")),
        }
    }
    false
}

// Windows 的 OpenSSH agent 走命名管道，尚未接入。
#[cfg(not(unix))]
async fn try_agent(
    _handle: &mut client::Handle<ClientHandler>,
    _user: &str,
    _rsa_hash: Option<HashAlg>,
    _tried: &mut Vec<String>,
) -> bool {
    false
}
