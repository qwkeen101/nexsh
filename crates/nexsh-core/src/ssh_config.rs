//! 读取 OpenSSH 的 `~/.ssh/config`，把具体的 Host 别名转换成主机列表。
//!
//! 只处理 `Host` 块里的 HostName / User / Port / IdentityFile。带通配符的别名、
//! `Match` 块和 `Include` 目前都跳过。

use std::path::{Path, PathBuf};

use crate::host::{AuthMethod, Host, default_user};

/// 从 OpenSSH 配置读到的主机在侧栏里归到这个分组。名称里不能有 `/`，那是分组的层级分隔符。
pub const GROUP: &str = "SSH 配置";

/// 默认的配置文件位置。
pub fn default_path() -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join(".ssh").join("config"))
}

/// 读取并解析；文件不存在或读不了时返回空列表。
pub fn load(path: &Path) -> Vec<Host> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text, std::env::home_dir().as_deref()),
        Err(_) => Vec::new(),
    }
}

#[derive(Default)]
struct Block {
    aliases: Vec<String>,
    hostname: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity_file: Option<PathBuf>,
}

pub fn parse(text: &str, home: Option<&Path>) -> Vec<Host> {
    let mut hosts = Vec::new();
    let mut block: Option<Block> = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = split_key_value(line) else { continue };
        match key.to_ascii_lowercase().as_str() {
            "host" => {
                flush(block.take(), &mut hosts);
                let aliases = value
                    .split_whitespace()
                    .filter(|alias| !alias.contains(['*', '?', '!']))
                    .map(str::to_string)
                    .collect();
                block = Some(Block { aliases, ..Block::default() });
            }
            // Match 结束它上面的 Host 块，自身不产生主机。
            "match" => flush(block.take(), &mut hosts),
            // OpenSSH 的规则是先出现的值生效。
            "hostname" => set_once(&mut block, |b| &mut b.hostname, || value.to_string()),
            "user" => set_once(&mut block, |b| &mut b.user, || value.to_string()),
            "port" => {
                if let Ok(port) = value.parse() {
                    set_once(&mut block, |b| &mut b.port, || port);
                }
            }
            "identityfile" => {
                set_once(&mut block, |b| &mut b.identity_file, || expand_home(value, home))
            }
            _ => {}
        }
    }
    flush(block, &mut hosts);
    hosts
}

fn set_once<T>(
    block: &mut Option<Block>,
    field: impl FnOnce(&mut Block) -> &mut Option<T>,
    value: impl FnOnce() -> T,
) {
    if let Some(block) = block {
        let slot = field(block);
        if slot.is_none() {
            *slot = Some(value());
        }
    }
}

fn flush(block: Option<Block>, hosts: &mut Vec<Host>) {
    let Some(block) = block else { return };
    for alias in &block.aliases {
        if hosts.iter().any(|h: &Host| &h.name == alias) {
            continue;
        }
        hosts.push(Host {
            name: alias.clone(),
            hostname: block.hostname.clone().unwrap_or_else(|| alias.clone()),
            port: block.port.unwrap_or(22),
            user: block.user.clone().unwrap_or_else(default_user),
            identity_file: block.identity_file.clone(),
            group: Some(GROUP.to_string()),
            auth: AuthMethod::Auto,
            note: None,
        });
    }
}

/// 支持 `Key Value`、`Key=Value`、`Key = Value` 和带引号的值。
fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let split_at = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let key = &line[..split_at];
    let value = line[split_at..].trim_start_matches(|c: char| c.is_whitespace() || c == '=');
    let value = value.trim().trim_matches('"');
    (!key.is_empty() && !value.is_empty()).then_some((key, value))
}

fn expand_home(value: &str, home: Option<&Path>) -> PathBuf {
    match (value.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# comment
Host *
    ServerAliveInterval 30

Host build-box bb
    HostName 10.0.0.5
    User deploy
    Port 2222
    IdentityFile ~/.ssh/build_key
    User ignored-second-value

Host gpu
    HostName=gpu.example.com
    User = "ml"

Match originalhost hidden
    HostName hidden.example.com

Host bare
"#;

    #[test]
    fn parses_concrete_hosts() {
        let hosts = parse(SAMPLE, Some(Path::new("/home/me")));
        let names: Vec<_> = hosts.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["build-box", "bb", "gpu", "bare"]);

        let build = &hosts[0];
        assert_eq!(build.hostname, "10.0.0.5");
        assert_eq!(build.user, "deploy");
        assert_eq!(build.port, 2222);
        assert_eq!(build.identity_file.as_deref(), Some(Path::new("/home/me/.ssh/build_key")));

        let gpu = &hosts[2];
        assert_eq!(gpu.hostname, "gpu.example.com");
        assert_eq!(gpu.user, "ml");
        assert_eq!(gpu.port, 22);

        // 没有 HostName 的别名连接到它自己的名字。
        assert_eq!(hosts[3].hostname, "bare");
    }

    #[test]
    fn match_block_adds_nothing() {
        let hosts = parse("Match host x\n  HostName y\n", None);
        assert!(hosts.is_empty());
    }
}
