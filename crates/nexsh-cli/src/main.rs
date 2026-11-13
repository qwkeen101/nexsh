//! 命令行工具：管理主机列表，以及在当前终端里直接连接。
//! 主要用来在不启动界面的情况下验证 nexsh-core 的 SSH 实现。

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use nexsh_core::{
    ConnectOptions, Host, HostStore, SessionCommand, SessionEvent, SshError, spawn_session,
    ssh_config,
};

const USAGE: &str = "\
用法:
  nexsh-cli host list
  nexsh-cli host add <名称> <user@host[:port]> [-i 私钥路径]
  nexsh-cli host rm <名称>
  nexsh-cli connect <名称 | user@host[:port]> [-i 私钥路径]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["host", "list"] => host_list(),
        ["host", "add", name, target, rest @ ..] => host_add(name, target, rest),
        ["host", "rm", name] => host_rm(name),
        ["connect", target, rest @ ..] => connect(target, rest),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(code) => code,
        Err(message) => {
            eprintln!("nexsh-cli: {message}");
            ExitCode::FAILURE
        }
    }
}

/// 解析可选的 `-i <路径>`。
fn identity_arg(rest: &[&str]) -> Result<Option<PathBuf>, String> {
    match rest {
        [] => Ok(None),
        ["-i", path] => Ok(Some(PathBuf::from(path))),
        _ => Err(format!("无法识别的参数: {}\n{USAGE}", rest.join(" "))),
    }
}

fn imported_hosts() -> Vec<Host> {
    ssh_config::default_path().map(|path| ssh_config::load(&path)).unwrap_or_default()
}

fn host_list() -> Result<ExitCode, String> {
    let store = HostStore::load_default().map_err(|e| e.to_string())?;
    let imported = imported_hosts();
    if store.hosts().is_empty() && imported.is_empty() {
        println!("还没有主机。用 `nexsh-cli host add` 添加。");
        return Ok(ExitCode::SUCCESS);
    }
    for host in store.hosts() {
        println!("{:<20} {}", host.name, host.address());
    }
    for host in imported.iter().filter(|h| store.find(&h.name).is_none()) {
        println!("{:<20} {}  (~/.ssh/config)", host.name, host.address());
    }
    Ok(ExitCode::SUCCESS)
}

fn host_add(name: &str, target: &str, rest: &[&str]) -> Result<ExitCode, String> {
    let mut host =
        Host::parse_target(target).ok_or_else(|| format!("无法解析地址: {target}"))?;
    host.name = name.to_string();
    host.identity_file = identity_arg(rest)?;
    let mut store = HostStore::load_default().map_err(|e| e.to_string())?;
    store.add(host).map_err(|e| e.to_string())?;
    store.save().map_err(|e| e.to_string())?;
    println!("已保存到 {}", store.path().display());
    Ok(ExitCode::SUCCESS)
}

fn host_rm(name: &str) -> Result<ExitCode, String> {
    let mut store = HostStore::load_default().map_err(|e| e.to_string())?;
    store.remove(name).map_err(|e| e.to_string())?;
    store.save().map_err(|e| e.to_string())?;
    println!("已删除 {name}");
    Ok(ExitCode::SUCCESS)
}

/// 先按名称在已保存主机和 ~/.ssh/config 里找，找不到再当作地址解析。
fn resolve_host(target: &str) -> Result<Host, String> {
    let store = HostStore::load_default().map_err(|e| e.to_string())?;
    if let Some(host) = store.find(target) {
        return Ok(host.clone());
    }
    if let Some(host) = imported_hosts().into_iter().find(|h| h.name == target) {
        return Ok(host);
    }
    Host::parse_target(target).ok_or_else(|| format!("找不到主机，也无法解析为地址: {target}"))
}

fn ask_secret(prompt: String) -> Result<String, String> {
    rpassword::prompt_password(prompt).map_err(|e| e.to_string())
}

fn connect(target: &str, rest: &[&str]) -> Result<ExitCode, String> {
    let mut host = resolve_host(target)?;
    if let Some(identity) = identity_arg(rest)? {
        host.identity_file = Some(identity);
    }
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut options = ConnectOptions::new(host, cols as u32, rows as u32);

    // 服务器要密码或私钥要口令时，问过之后重连；总共最多重试 5 次。
    for _ in 0..6 {
        eprintln!("正在连接 {} ...", options.host.address());
        match run_session(options.clone()) {
            Ok(code) => return Ok(code),
            Err(SshError::PasswordRequired) => {
                options.password =
                    Some(ask_secret(format!("{} 的密码: ", options.host.address()))?);
            }
            Err(SshError::AuthFailed { .. }) if options.password.is_some() => {
                eprintln!("密码不正确。");
                options.password =
                    Some(ask_secret(format!("{} 的密码: ", options.host.address()))?);
            }
            Err(SshError::PassphraseRequired(path)) => {
                options.key_passphrase =
                    Some(ask_secret(format!("私钥 {} 的口令: ", path.display()))?);
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("认证失败次数过多".to_string())
}

/// 退出时恢复终端模式，即使中途出错。
struct RawMode;

impl RawMode {
    fn enable() -> Option<Self> {
        crossterm::terminal::enable_raw_mode().ok().map(|_| RawMode)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

fn run_session(options: ConnectOptions) -> Result<ExitCode, SshError> {
    let mut size = (options.cols as u16, options.rows as u16);
    let mut session = spawn_session(options);
    let commands = session.commands.clone();
    let mut raw_mode = None;
    let mut exit_code = ExitCode::SUCCESS;
    let mut failure = None;

    nexsh_core::runtime().block_on(async {
        let mut resize_poll = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {
                event = session.events.recv() => match event {
                    Some(SessionEvent::HostKeyLearned(fingerprint)) => {
                        eprintln!("首次连接，已记住主机密钥 {fingerprint}");
                    }
                    Some(SessionEvent::Connected) => {
                        raw_mode = RawMode::enable();
                        spawn_stdin_reader(commands.clone());
                    }
                    Some(SessionEvent::Output(bytes)) => {
                        let mut stdout = std::io::stdout().lock();
                        let _ = stdout.write_all(&bytes);
                        let _ = stdout.flush();
                    }
                    Some(SessionEvent::Exited(code)) => {
                        exit_code = ExitCode::from(code.unwrap_or(0).min(255) as u8);
                    }
                    Some(SessionEvent::Failed(error)) => failure = Some(error),
                    // 延迟只在图形界面里显示。
                    Some(SessionEvent::Latency(_)) => {}
                    Some(SessionEvent::Closed) | None => break,
                },
                _ = resize_poll.tick(), if raw_mode.is_some() => {
                    if let Ok(current) = crossterm::terminal::size()
                        && current != size
                    {
                        size = current;
                        let _ = commands.send(SessionCommand::Resize {
                            cols: current.0 as u32,
                            rows: current.1 as u32,
                        });
                    }
                }
            }
        }
    });

    drop(raw_mode);
    match failure {
        Some(error) => Err(error),
        None => Ok(exit_code),
    }
}

/// 标准输入是阻塞读，放在独立线程里；会话结束后线程随进程退出。
fn spawn_stdin_reader(commands: tokio::sync::mpsc::UnboundedSender<SessionCommand>) {
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buffer = [0u8; 4096];
        loop {
            match stdin.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if commands.send(SessionCommand::Input(buffer[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    });
}
