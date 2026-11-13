//! 主机搜索与快速连接：侧栏的搜索框既用来筛选已有主机，也能直接输入一个地址连接。

use std::path::PathBuf;

use crate::host::Host;

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

/// 把查询当成一个连接目标来解析。接受的写法：
/// `user@host`、`host:port`、`ssh://user@host:port`，以及带 `-p`、`-l`、`-i` 的
/// `ssh` 命令行。出现其他选项或多余的参数时返回 None：
/// 宁可不识别，也不猜一个可能连错地方的目标。
pub fn quick_target(query: &str) -> Option<Host> {
    let mut words = query.split_whitespace().peekable();
    if words.peek() == Some(&"ssh") {
        words.next();
    }

    let mut target = None;
    let mut port = None;
    let mut user = None;
    let mut identity_file = None;
    while let Some(word) = words.next() {
        if let Some(option) = word.strip_prefix('-') {
            // 选项的值可以紧跟在字母后面（-p2222），也可以是下一个词（-p 2222）。
            let mut chars = option.chars();
            let flag = chars.next()?;
            let attached = chars.as_str();
            let value = if attached.is_empty() { words.next()? } else { attached };
            match flag {
                'p' => port = Some(value.parse::<u16>().ok()?),
                'l' => user = Some(value.to_string()),
                'i' => identity_file = Some(PathBuf::from(value)),
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
    let mut host = Host::parse_target(target)?;
    // 主机名里不会有这些字符；有的话多半是用户还在打别的东西。
    if host.hostname.contains(['/', '\\', '"', '\'']) {
        return None;
    }
    if let Some(port) = port {
        host.port = port;
    }
    if let Some(user) = user {
        host.user = user;
    }
    host.identity_file = identity_file;
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
