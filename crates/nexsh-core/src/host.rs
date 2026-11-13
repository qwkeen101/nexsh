use std::path::PathBuf;

use serde::{Deserialize, Serialize};

fn default_port() -> u16 {
    22
}

/// 登录时用哪种凭据。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    /// 依次尝试 ssh-agent、私钥文件，都不行再问密码。
    #[default]
    Auto,
    /// 只用密码。
    Password,
    /// 只用指定的私钥文件。
    Key,
}

impl AuthMethod {
    fn is_auto(&self) -> bool {
        *self == AuthMethod::Auto
    }
}

/// 一台可连接的主机。密码和口令不保存在这里，而是在系统钥匙串里（见 [`crate::secrets`]）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    /// 显示名称，在主机列表里唯一。
    pub name: String,
    /// 域名或 IP。
    pub hostname: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "AuthMethod::is_auto")]
    pub auth: AuthMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Host {
    /// 解析 `user@host:port`、`user@host`、`host:port`、`host`，以及 `[ipv6]:port`。
    pub fn parse_target(target: &str) -> Option<Host> {
        let target = target.trim();
        if target.is_empty() {
            return None;
        }
        let (user, rest) = match target.rsplit_once('@') {
            Some((user, rest)) if !user.is_empty() => (user.to_string(), rest),
            Some(_) => return None,
            None => (default_user(), target),
        };
        let (hostname, port) = split_host_port(rest)?;
        Some(Host {
            name: target.to_string(),
            hostname,
            port: port.unwrap_or(22),
            user,
            identity_file: None,
            group: None,
            auth: AuthMethod::Auto,
            note: None,
        })
    }

    /// `user@host` 或 `user@host:port` 形式的地址。
    pub fn address(&self) -> String {
        if self.port == 22 {
            format!("{}@{}", self.user, self.hostname)
        } else {
            format!("{}@{}:{}", self.user, self.hostname, self.port)
        }
    }
}

/// 拆出主机名和端口；没有写端口时端口为 None，由调用方决定默认值。
pub(crate) fn split_host_port(rest: &str) -> Option<(String, Option<u16>)> {
    if let Some(inner) = rest.strip_prefix('[') {
        let (host, tail) = inner.split_once(']')?;
        let port = match tail.strip_prefix(':') {
            Some(port) => Some(port.parse().ok()?),
            None if tail.is_empty() => None,
            None => return None,
        };
        return (!host.is_empty()).then(|| (host.to_string(), port));
    }
    // 多于一个冒号且没有方括号，按裸 IPv6 地址处理。
    if rest.matches(':').count() > 1 {
        return Some((rest.to_string(), None));
    }
    match rest.split_once(':') {
        Some((host, port)) if !host.is_empty() => {
            Some((host.to_string(), Some(port.parse().ok()?)))
        }
        Some(_) => None,
        None if rest.is_empty() => None,
        None => Some((rest.to_string(), None)),
    }
}

/// 把开头的 `~/` 换成用户主目录。
pub fn expand_home(path: &str) -> PathBuf {
    let rest = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"));
    match (rest, std::env::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

/// 当前系统用户名，取不到时回退为 root。
pub fn default_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "root".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_target() {
        let host = Host::parse_target("deploy@example.com:2222").unwrap();
        assert_eq!(host.user, "deploy");
        assert_eq!(host.hostname, "example.com");
        assert_eq!(host.port, 2222);
        assert_eq!(host.address(), "deploy@example.com:2222");
    }

    #[test]
    fn defaults_port_to_22() {
        let host = Host::parse_target("root@10.0.0.1").unwrap();
        assert_eq!(host.port, 22);
        assert_eq!(host.address(), "root@10.0.0.1");
    }

    #[test]
    fn parses_ipv6() {
        let host = Host::parse_target("root@[::1]:2200").unwrap();
        assert_eq!(host.hostname, "::1");
        assert_eq!(host.port, 2200);
        let bare = Host::parse_target("root@fe80::1").unwrap();
        assert_eq!(bare.hostname, "fe80::1");
        assert_eq!(bare.port, 22);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Host::parse_target("").is_none());
        assert!(Host::parse_target("@host").is_none());
        assert!(Host::parse_target("user@host:notaport").is_none());
        assert!(Host::parse_target("user@:22").is_none());
    }
}
