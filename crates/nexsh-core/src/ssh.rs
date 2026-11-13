//! SSH 会话：连接、主机密钥校验、认证、带 PTY 的交互式 shell。
//!
//! 会话跑在 [`crate::runtime`] 上，通过两个通道和调用方通信，调用方可以是任何
//! 执行器（界面线程不需要 tokio）。事件通道有容量上限：界面来不及消费时，
//! 这里会停止读取 SSH 通道，压力一路传回服务器，内存不会无限增长。

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use russh::keys::known_hosts::{check_known_hosts_path, learn_known_hosts_path};
use russh::keys::{self, HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect, client};
use tokio::sync::mpsc;

use crate::host::{AuthMethod, Host};
use crate::secrets::{self, SecretKind};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
/// 多久测一次往返延迟，以及等回应的时限。时限短于间隔，同一时刻最多一个探测在途。
const LATENCY_INTERVAL: Duration = Duration::from_secs(5);
const LATENCY_TIMEOUT: Duration = Duration::from_secs(4);
/// 未被消费的事件上限，超过后对服务器施加背压。
const EVENT_QUEUE_CAPACITY: usize = 256;
/// 服务器通常在 6 次失败后断开（MaxAuthTries），给密钥文件和密码留出余量。
#[cfg(unix)]
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
    #[error("{0}有口令保护，需要输入口令")]
    PassphraseRequired(KeySource),
    #[error("密钥认证未通过，需要输入密码")]
    PasswordRequired,
    #[error("认证失败，已尝试: {}", .tried.join("、"))]
    AuthFailed { tried: Vec<String> },
    #[error("SSH 协议错误: {0}")]
    Protocol(#[from] russh::Error),
}

/// 一把私钥从哪里来。显示出来是"私钥 /路径"或"导入的私钥"，后面可以直接接谓语。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeySource {
    File(PathBuf),
    /// 内容由调用方直接给出（[`ConnectOptions::identity`]），没有对应的文件。
    Imported,
}

impl fmt::Display for KeySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeySource::File(path) => write!(f, "私钥 {} ", path.display()),
            KeySource::Imported => f.write_str("导入的私钥"),
        }
    }
}

/// 一把私钥的公开信息，导入时给用户核对用。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyInfo {
    /// 算法名，如 `ssh-ed25519`。
    pub algorithm: String,
    /// 公钥的 SHA256 指纹，和 `ssh-keygen -l` 显示的一致。
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyProblem {
    /// 有口令保护，而且没有给口令。
    Encrypted,
    WrongPassphrase,
    /// 不是能识别的私钥；携带解析器给出的原因。
    Invalid(String),
}

/// 检查一段文字是不是可用的私钥。口令为空等同于没给。
pub fn inspect_key(text: &str, passphrase: Option<&str>) -> Result<KeyInfo, KeyProblem> {
    let passphrase = passphrase.filter(|passphrase| !passphrase.is_empty());
    match decode_key(text, None) {
        Ok(key) => Ok(key_info(&key)),
        Err(keys::Error::KeyIsEncrypted) => match passphrase {
            None => Err(KeyProblem::Encrypted),
            Some(passphrase) => decode_key(text, Some(passphrase))
                .map(|key| key_info(&key))
                .map_err(|_| KeyProblem::WrongPassphrase),
        },
        Err(error) => Err(KeyProblem::Invalid(error.to_string())),
    }
}

fn key_info(key: &keys::PrivateKey) -> KeyInfo {
    KeyInfo {
        algorithm: key.algorithm().as_str().to_string(),
        fingerprint: key.public_key().fingerprint(HashAlg::Sha256).to_string(),
    }
}

/// 解析私钥文字。从聊天软件、网页里复制出来的私钥常带着行首行尾的空白和 Windows 换行，
/// 解析器按整行比对 BEGIN/END 标记，所以先逐行去掉空白。
fn decode_key(text: &str, passphrase: Option<&str>) -> Result<keys::PrivateKey, keys::Error> {
    let mut clean = String::with_capacity(text.len() + 1);
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        clean.push_str(line);
        clean.push('\n');
    }
    // PuTTY 格式的加密私钥在没给口令时报的是普通的解析错误，这里统一成"需要口令"。
    let putty_encrypted = clean.starts_with("PuTTY-User-Key-File-")
        && clean.lines().any(|line| {
            line.strip_prefix("Encryption:").is_some_and(|value| value.trim() != "none")
        });
    if putty_encrypted && passphrase.is_none() {
        return Err(keys::Error::KeyIsEncrypted);
    }
    keys::decode_secret_key(&clean, passphrase)
}

/// 遇到 known_hosts 里没有的主机时怎么办。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HostKeyPolicy {
    /// 接受并写入 known_hosts（相当于 OpenSSH 的 accept-new）。
    #[default]
    AcceptNew,
    /// 拒绝。
    Strict,
    /// 接受但不记录。测试连接时用：只是试一下，不应该改动 known_hosts。
    AcceptOnce,
}

#[derive(Clone, Debug)]
pub struct ConnectOptions {
    pub host: Host,
    pub cols: u32,
    pub rows: u32,
    pub password: Option<String>,
    /// 私钥口令。空字符串表示跳过有口令保护的私钥。
    pub key_passphrase: Option<String>,
    /// 私钥内容。给出时只用它，不再去读私钥文件：手机上私钥加密保存在应用里，不落成明文文件。
    pub identity: Option<String>,
    pub host_key_policy: HostKeyPolicy,
    /// 为 None 时使用 `~/.ssh/known_hosts`。
    pub known_hosts_path: Option<PathBuf>,
    /// 没有给出密码或口令时，到系统钥匙串里按这个主机名称查找保存的；None 表示不查。
    pub saved_secrets: Option<String>,
}

impl ConnectOptions {
    pub fn new(host: Host, cols: u32, rows: u32) -> Self {
        Self {
            saved_secrets: Some(host.name.clone()),
            host,
            cols,
            rows,
            password: None,
            key_passphrase: None,
            identity: None,
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
    /// 一次往返延迟的测量结果；None 表示在时限内没有得到回应。
    Latency(Option<Duration>),
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
                    HostKeyPolicy::AcceptOnce => Ok(Some(fingerprint)),
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

/// 测试连接的结果。
#[derive(Clone, Debug)]
pub struct ProbeReport {
    /// 从开始连接到登录成功用了多久。
    pub elapsed: Duration,
    /// 这台主机不在 known_hosts 里时，它的密钥指纹。
    pub new_host_key: Option<String>,
}

/// 用给定的参数登录一次然后断开，不打开终端，也不改动 known_hosts。
/// 立即返回，结果从通道里取；丢掉通道不会让后台的尝试提前结束，它最多持续到连接超时。
pub fn spawn_probe(
    mut options: ConnectOptions,
) -> tokio::sync::oneshot::Receiver<Result<ProbeReport, SshError>> {
    if options.host_key_policy == HostKeyPolicy::AcceptNew {
        options.host_key_policy = HostKeyPolicy::AcceptOnce;
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    crate::runtime().spawn(async move {
        let started = Instant::now();
        let outcome = async {
            let (handle, new_host_key) = establish(options).await?;
            let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
            Ok::<_, SshError>(ProbeReport { elapsed: started.elapsed(), new_host_key })
        }
        .await;
        let _ = sender.send(outcome);
    });
    receiver
}

/// 没有给出的密码和口令，到钥匙串里找保存的。钥匙串读不了时当作没有保存，
/// 之后照常向用户询问。
async fn with_saved_secrets(mut options: ConnectOptions) -> ConnectOptions {
    let Some(name) = options.saved_secrets.clone() else { return options };
    let need_password = options.password.is_none() && options.host.auth != AuthMethod::Key;
    let need_passphrase =
        options.key_passphrase.is_none() && options.host.auth != AuthMethod::Password;
    if !need_password && !need_passphrase {
        return options;
    }
    // 读钥匙串可能要等系统弹窗解锁，不能占着异步线程。
    let lookup = tokio::task::spawn_blocking(move || {
        let read = |wanted: bool, kind| {
            wanted.then(|| secrets::load(kind, &name).ok().flatten()).flatten()
        };
        (read(need_password, SecretKind::Password), read(need_passphrase, SecretKind::Passphrase))
    });
    if let Ok((password, passphrase)) = lookup.await {
        options.password = options.password.or(password);
        options.key_passphrase = options.key_passphrase.or(passphrase);
    }
    options
}

/// 建立连接、校验主机密钥并完成认证。返回连接，以及首次见到这台主机时它的密钥指纹。
async fn establish(
    options: ConnectOptions,
) -> Result<(client::Handle<ClientHandler>, Option<String>), SshError> {
    let options = with_saved_secrets(options).await;
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
    authenticate(&mut handle, &options).await?;
    Ok((handle, learned))
}

async fn run_session(
    options: ConnectOptions,
    mut commands: mpsc::UnboundedReceiver<SessionCommand>,
    events: &mpsc::Sender<SessionEvent>,
) -> Result<(), SshError> {
    let (cols, rows) = (options.cols, options.rows);
    let learning = options.host_key_policy == HostKeyPolicy::AcceptNew;
    let (handle, new_host_key) = establish(options).await?;
    if let Some(fingerprint) = new_host_key.filter(|_| learning) {
        let _ = events.send(SessionEvent::HostKeyLearned(fingerprint)).await;
    }

    let mut channel = handle.channel_open_session().await?;
    channel.request_pty(false, TERM, cols, rows, 0, 0, &[]).await?;
    channel.request_shell(true).await?;
    if events.send(SessionEvent::Connected).await.is_err() {
        return Ok(());
    }

    let mut exit_status = None;
    let mut probe_timer = tokio::time::interval(LATENCY_INTERVAL);
    probe_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // 在途的延迟探测。它和终端数据走同一条连接，放在同一个循环里轮询，不另开任务。
    let mut probe = None;
    let connection = &handle;
    loop {
        tokio::select! {
            _ = probe_timer.tick(), if probe.is_none() => {
                let started = Instant::now();
                probe = Some(Box::pin(async move {
                    match tokio::time::timeout(LATENCY_TIMEOUT, connection.send_ping()).await {
                        Ok(Ok(())) => Some(started.elapsed()),
                        _ => None,
                    }
                }));
            }
            sample = async {
                match probe.as_mut() {
                    Some(probe) => probe.await,
                    None => std::future::pending().await,
                }
            }, if probe.is_some() => {
                probe = None;
                // 延迟只是参考信息：界面来不及处理时丢掉这个样本，不为它阻塞终端数据。
                let _ = events.try_send(SessionEvent::Latency(sample));
            }
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

/// 自动模式依次尝试 ssh-agent、私钥文件、密码；指定了认证方式时只用那一种。
async fn authenticate(
    handle: &mut client::Handle<ClientHandler>,
    options: &ConnectOptions,
) -> Result<(), SshError> {
    let user = options.host.user.as_str();
    let rsa_hash = handle.best_supported_rsa_hash().await?.flatten();
    let mut tried = Vec::new();

    let method = options.host.auth;
    if method == AuthMethod::Auto && try_agent(handle, user, rsa_hash, &mut tried).await {
        return Ok(());
    }

    let mut encrypted_key = None;
    let sources = if method == AuthMethod::Password { Vec::new() } else { key_sources(options) };
    for source in sources {
        let label = source.to_string().trim_end().to_string();
        let text = match &source {
            KeySource::File(path) => std::fs::read_to_string(path).map_err(keys::Error::from),
            KeySource::Imported => Ok(options.identity.clone().unwrap_or_default()),
        };
        // 先不带口令读：没加密的私钥不受口令影响，加密的再用口令重试。
        let loaded = text.and_then(|text| match decode_key(&text, None) {
            Err(keys::Error::KeyIsEncrypted) => Ok(Err(text)),
            other => other.map(Ok),
        });
        let loaded = match loaded {
            Ok(Err(text)) => match options.key_passphrase.as_deref() {
                None => {
                    encrypted_key.get_or_insert(source);
                    continue;
                }
                // 空口令表示用户选择跳过加密的私钥。
                Some("") => continue,
                Some(passphrase) => match decode_key(&text, Some(passphrase)) {
                    Ok(key) => Ok(key),
                    // 口令不对：让调用方重新询问，而不是悄悄改用密码。
                    Err(_) => return Err(SshError::PassphraseRequired(source)),
                },
            },
            Ok(Ok(key)) => Ok(key),
            Err(error) => Err(error),
        };
        match loaded {
            Ok(key) => {
                let key = PrivateKeyWithHashAlg::new(Arc::new(key), rsa_hash);
                if handle.authenticate_publickey(user, key).await?.success() {
                    return Ok(());
                }
                tried.push(label);
            }
            Err(error) => tried.push(format!("{label}（{error}）")),
        }
    }

    if method == AuthMethod::Key {
        // 只用私钥：不退回密码，免得用户以为私钥生效了。
        return Err(match encrypted_key {
            Some(source) => SshError::PassphraseRequired(source),
            None => SshError::AuthFailed { tried },
        });
    }

    if let Some(password) = &options.password {
        if handle.authenticate_password(user, password.as_str()).await?.success() {
            return Ok(());
        }
        tried.push("密码".to_string());
    }

    // 还有没用上的凭据时，告诉调用方该向用户要什么。
    if let Some(source) = encrypted_key {
        return Err(SshError::PassphraseRequired(source));
    }
    if options.password.is_none() {
        return Err(SshError::PasswordRequired);
    }
    Err(SshError::AuthFailed { tried })
}

/// 直接给了私钥内容或主机指定了私钥文件时只用那一把，否则按 OpenSSH 的默认顺序找。
fn key_sources(options: &ConnectOptions) -> Vec<KeySource> {
    if options.identity.is_some() {
        return vec![KeySource::Imported];
    }
    if let Some(path) = &options.host.identity_file {
        return vec![KeySource::File(path.clone())];
    }
    let Some(home) = std::env::home_dir() else { return Vec::new() };
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .into_iter()
        .map(|name| home.join(".ssh").join(name))
        .filter(|path| path.is_file())
        .map(KeySource::File)
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

#[cfg(test)]
mod tests {
    use super::*;

    // 专为测试生成的密钥，没有在任何服务器上授权过。
    const PLAIN_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\n\
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\n\
QyNTUxOQAAACCwLzQR+Vm5cGy4IL28PkZgk0bqgqL0t78JDY3XIt6BHQAAAJAtWtwcLVrc\n\
HAAAAAtzc2gtZWQyNTUxOQAAACCwLzQR+Vm5cGy4IL28PkZgk0bqgqL0t78JDY3XIt6BHQ\n\
AAAEBSvIuez8jkC5u2WLcTErl0K8xymT1HBYXhZ3qL9Gwsd7AvNBH5WblwbLggvbw+RmCT\n\
RuqCovS3vwkNjdci3oEdAAAACm5leHNoLXRlc3QBAgM=\n\
-----END OPENSSH PRIVATE KEY-----\n";
    /// 口令是 nexsh。
    const ENCRYPTED_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\n\
b3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jdHIAAAAGYmNyeXB0AAAAGAAAABD7iWZmoJ\n\
V3vCVGAuEN3v1JAAAAAgAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAINKMVNEnZDCoPgub\n\
UBAktOwNzSDSW6sJpAmk63V2///LAAAAkCbgKcyJujrfdqyD8wWAerfF7qgM9LnIUbP1qO\n\
Qoyi9ulFExogs0G3MDzIcqghqxflBL+YMzOYbAWB+9E8AErgWF8XkC6Qd2utLOivbXbKUd\n\
5hv4SYXaFBYlNvi5gQjpbg5Mc3LP0NiR7grOm3M9aLXki5fo5/IrFrZfiQivsF7bHn41xV\n\
aC1iNWDhHq2vl8dA==\n\
-----END OPENSSH PRIVATE KEY-----\n";

    #[test]
    fn describes_a_plain_key() {
        let info = inspect_key(PLAIN_KEY, None).unwrap();
        assert_eq!(info.algorithm, "ssh-ed25519");
        assert_eq!(info.fingerprint, "SHA256:HNqfXvoiwH/1RzepruUjvTB2l6kyum10jGTbbo+AUWM");
    }

    #[test]
    fn tolerates_pasted_whitespace() {
        let pasted: String =
            PLAIN_KEY.lines().map(|line| format!("  {line} \r\n\r\n")).collect();
        assert_eq!(inspect_key(&pasted, None), inspect_key(PLAIN_KEY, None));
    }

    #[test]
    fn encrypted_keys_need_the_right_passphrase() {
        assert_eq!(inspect_key(ENCRYPTED_KEY, None), Err(KeyProblem::Encrypted));
        assert_eq!(inspect_key(ENCRYPTED_KEY, Some("")), Err(KeyProblem::Encrypted));
        assert_eq!(inspect_key(ENCRYPTED_KEY, Some("wrong")), Err(KeyProblem::WrongPassphrase));
        assert_eq!(inspect_key(ENCRYPTED_KEY, Some("nexsh")).unwrap().algorithm, "ssh-ed25519");
    }

    #[test]
    fn rejects_text_that_is_not_a_key() {
        assert!(matches!(inspect_key("ssh-ed25519 AAAA", None), Err(KeyProblem::Invalid(_))));
        assert!(matches!(inspect_key("", None), Err(KeyProblem::Invalid(_))));
    }

    #[test]
    fn an_imported_key_takes_precedence_over_files() {
        let mut host = Host::parse_target("a@b").unwrap();
        host.identity_file = Some(PathBuf::from("/keys/a"));
        let mut options = ConnectOptions::new(host, 80, 24);
        assert_eq!(key_sources(&options), vec![KeySource::File(PathBuf::from("/keys/a"))]);
        options.identity = Some(PLAIN_KEY.to_string());
        assert_eq!(key_sources(&options), vec![KeySource::Imported]);
        assert_eq!(
            SshError::PassphraseRequired(KeySource::Imported).to_string(),
            "导入的私钥有口令保护，需要输入口令"
        );
    }
}
