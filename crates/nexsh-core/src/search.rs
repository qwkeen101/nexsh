//! 主机搜索与快速连接：侧栏的搜索框既用来筛选已有主机，也能直接输入一个地址连接。

use std::path::PathBuf;

use crate::host::{AuthMethod, Host, default_user, split_host_port};

/// 查询按空白切成若干个词，每个词都要出现在主机的某个字段里（不区分大小写）。
/// 空查询匹配所有主机。
pub fn matches(host: &Host, query: &str) -> bool {
    let haystack = [
        host.name.to_lowercase(),
        host.address().to_lowercase(),
        host.group.as_deref().unwrap_or_default().to_lowercase(),
    ];
    query
        .split_whitespace()
        .map(str::to_lowercase)
        .all(|term| haystack.iter().any(|field| field.contains(&term)))
}

/// 从一段文字里认出来的连接目标。没有明确写出的部分为 None。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Endpoint {
    pub hostname: String,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub identity_file: Option<PathBuf>,
}

/// 解析一个连接目标。接受的写法：
/// `user@host`、`host:port`、`ssh://user@host:port`，以及带 `-p`、`-l`、`-i` 的
/// `ssh` 命令行。出现其他选项或多余的参数时返回 None：
/// 宁可不识别，也不猜一个可能连错地方的目标。
pub fn parse_endpoint(text: &str) -> Option<Endpoint> {
    let mut words = text.split_whitespace().peekable();
    if words.peek() == Some(&"ssh") {
        words.next();
    }

    let mut target = None;
    let mut endpoint = Endpoint::default();
    while let Some(word) = words.next() {
        if let Some(option) = word.strip_prefix('-') {
            // 选项的值可以紧跟在字母后面（-p2222），也可以是下一个词（-p 2222）。
            let mut chars = option.chars();
            let flag = chars.next()?;
            let attached = chars.as_str();
            let value = if attached.is_empty() { words.next()? } else { attached };
            match flag {
                'p' => endpoint.port = Some(value.parse::<u16>().ok()?),
                'l' => endpoint.user = Some(value.to_string()),
                'i' => endpoint.identity_file = Some(PathBuf::from(value)),
                _ => return None,
            }
        } else if target.is_none() {
            target = Some(word);
        } else {
            return None;
        }
    }

    let target = target?;
    let target = target.strip_prefix("ssh://").map_or(target, |rest| rest.trim_end_matches('/'));
    let rest = match target.rsplit_once('@') {
        Some((user, rest)) if !user.is_empty() => {
            // 命令行里的 -l 优先于 user@ 前缀，与 OpenSSH 一致。
            endpoint.user.get_or_insert_with(|| user.to_string());
            rest
        }
        Some(_) => return None,
        None => target,
    };
    let (hostname, port) = split_host_port(rest)?;
    // 主机名里不会有这些字符；有的话多半是用户还在打别的东西。
    if hostname.contains(['/', '\\', '"', '\'']) {
        return None;
    }
    endpoint.hostname = hostname;
    endpoint.port = endpoint.port.or(port);
    Some(endpoint)
}

/// 把查询当成一个可以直接连接的主机；没写的用户名和端口取默认值。
pub fn quick_target(query: &str) -> Option<Host> {
    let endpoint = parse_endpoint(query)?;
    let mut host = Host {
        name: String::new(),
        hostname: endpoint.hostname,
        port: endpoint.port.unwrap_or(22),
        user: endpoint.user.unwrap_or_else(default_user),
        identity_file: endpoint.identity_file,
        group: None,
        auth: AuthMethod::Auto,
        note: None,
    };
    host.name = host.address();
    Some(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &str, target: &str, group: Option<&str>) -> Host {
        let mut host = Host::parse_target(target).unwrap();
        host.name = name.to_string();
        host.group = group.map(str::to_string);
        host
    }

    #[test]
    fn empty_query_matches_everything() {
        assert!(matches(&host("prod", "deploy@example.com", None), ""));
        assert!(matches(&host("prod", "deploy@example.com", None), "   "));
    }

    #[test]
    fn every_term_must_match_some_field() {
        let web = host("web-01", "deploy@10.0.1.11:2222", Some("生产环境"));
        assert!(matches(&web, "WEB"));
        assert!(matches(&web, "deploy 10.0"));
        assert!(matches(&web, "生产 web"));
        assert!(matches(&web, "deploy@10.0.1.11:2222"));
        assert!(!matches(&web, "web staging"));
    }

    #[test]
    fn parses_plain_targets() {
        let host = quick_target("root@10.0.0.5:2200").unwrap();
        assert_eq!((host.user.as_str(), host.hostname.as_str(), host.port), ("root", "10.0.0.5", 2200));
        assert_eq!(host.name, "root@10.0.0.5:2200");
        assert_eq!(quick_target("ssh://git@example.com:2222/").unwrap().port, 2222);
    }

    #[test]
    fn parses_ssh_command_lines() {
        let host = quick_target("ssh -p 2222 -l deploy example.com -i ~/.ssh/key").unwrap();
        assert_eq!((host.user.as_str(), host.hostname.as_str(), host.port), ("deploy", "example.com", 2222));
        assert_eq!(host.identity_file, Some(PathBuf::from("~/.ssh/key")));
        assert_eq!(quick_target("ssh -p2222 root@example.com").unwrap().port, 2222);
    }

    #[test]
    fn endpoint_keeps_track_of_what_was_written() {
        let bare = parse_endpoint("example.com").unwrap();
        assert_eq!(bare, Endpoint { hostname: "example.com".into(), ..Default::default() });
        let full = parse_endpoint("ssh -l admin root@[::1]:2200").unwrap();
        assert_eq!(full.hostname, "::1");
        assert_eq!(full.port, Some(2200));
        assert_eq!(full.user.as_deref(), Some("admin"));
    }

    #[test]
    fn refuses_to_guess() {
        assert!(quick_target("").is_none());
        assert!(quick_target("ssh").is_none());
        assert!(quick_target("ssh -J jump root@example.com").is_none());
        assert!(quick_target("ssh -p").is_none());
        assert!(quick_target("ssh -p abc root@example.com").is_none());
        assert!(quick_target("web 01").is_none());
        assert!(quick_target("root@").is_none());
    }
}
