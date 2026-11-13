//! 主机表单的内容和校验规则。界面只负责收集文字和显示错误，规则都在这里，可以单独测试。

use crate::host::{AuthMethod, Host, default_user, expand_home};
use crate::search::parse_endpoint;

/// 表单里可能出错的字段，界面据此把错误显示在对应的输入框下面。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Address,
    Port,
    User,
    Name,
    IdentityFile,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldError {
    pub field: Field,
    pub message: String,
}

/// 用户在表单里填的原始文字。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostInput {
    pub address: String,
    pub port: String,
    pub user: String,
    pub name: String,
    pub group: String,
    pub note: String,
    pub identity_file: String,
    pub auth: AuthMethod,
}

impl HostInput {
    pub fn from_host(host: &Host) -> Self {
        Self {
            address: host.hostname.clone(),
            port: host.port.to_string(),
            user: host.user.clone(),
            name: host.name.clone(),
            group: host.group.clone().unwrap_or_default(),
            note: host.note.clone().unwrap_or_default(),
            identity_file: host
                .identity_file
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            auth: host.auth,
        }
    }

    /// 地址栏里写的是 `user@host:port`、`ssh://` 链接或一条 ssh 命令时，
    /// 把各部分拆到对应的字段里。地址栏只剩主机名。返回是否改动了内容。
    ///
    /// 只覆盖文字里明确写出的部分：粘贴 `host:2222` 不会动已经填好的用户名。
    pub fn split_address(&mut self) -> bool {
        let Some(endpoint) = parse_endpoint(&self.address) else { return false };
        if endpoint.hostname == self.address.trim() {
            return false;
        }
        self.address = endpoint.hostname;
        if let Some(port) = endpoint.port {
            self.port = port.to_string();
        }
        if let Some(user) = endpoint.user {
            self.user = user;
        }
        if let Some(path) = endpoint.identity_file {
            self.identity_file = path.display().to_string();
            self.auth = AuthMethod::Key;
        }
        true
    }

    /// 校验并生成主机。`taken_names` 是其他主机已经占用的名称（编辑时不含它自己）。
    /// 一次返回所有字段的错误，用户不必改一处提交一次。
    pub fn build(&self, taken_names: &[String]) -> Result<Host, Vec<FieldError>> {
        let mut input = self.clone();
        input.split_address();
        let mut errors = Vec::new();
        let mut fail = |field, message: &str| {
            errors.push(FieldError { field, message: message.to_string() });
        };

        let hostname = input.address.trim();
        if hostname.is_empty() {
            fail(Field::Address, "请填写主机名或 IP 地址");
        } else if hostname.chars().any(|ch| ch.is_whitespace() || "/\\\"'@".contains(ch)) {
            fail(Field::Address, "地址里不能有空格、斜杠、引号或 @");
        }

        let port = match input.port.trim() {
            "" => 22,
            text => match text.parse::<u16>() {
                Ok(port) if port != 0 => port,
                _ => {
                    fail(Field::Port, "端口应为 1 到 65535 之间的数字");
                    0
                }
            },
        };

        let user = match input.user.trim() {
            "" => default_user(),
            text if text.chars().any(char::is_whitespace) => {
                fail(Field::User, "用户名里不能有空格");
                String::new()
            }
            text => text.to_string(),
        };

        let identity_file = match input.identity_file.trim() {
            "" => {
                if input.auth == AuthMethod::Key {
                    fail(Field::IdentityFile, "选择私钥认证时需要填写私钥文件");
                }
                None
            }
            text => {
                let path = expand_home(text);
                if !path.is_file() {
                    fail(Field::IdentityFile, "找不到这个文件");
                }
                Some(path)
            }
        };

        let mut host = Host {
            name: input.name.trim().to_string(),
            hostname: hostname.to_string(),
            port,
            user,
            // 密码认证用不到私钥，不把上次填的路径带进配置。
            identity_file: identity_file.filter(|_| input.auth != AuthMethod::Password),
            group: Some(input.group.trim().to_string()).filter(|group| !group.is_empty()),
            auth: input.auth,
            note: Some(input.note.trim().to_string()).filter(|note| !note.is_empty()),
        };
        if host.name.is_empty() {
            host.name = host.address();
        }
        if taken_names.contains(&host.name) {
            let message = if input.name.trim().is_empty() {
                "已有同名主机，请填写一个名称来区分"
            } else {
                "已有同名主机"
            };
            fail(Field::Name, message);
        }

        if errors.is_empty() { Ok(host) } else { Err(errors) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(address: &str) -> HostInput {
        HostInput { address: address.to_string(), ..Default::default() }
    }

    fn fields(result: Result<Host, Vec<FieldError>>) -> Vec<Field> {
        result.unwrap_err().into_iter().map(|error| error.field).collect()
    }

    #[test]
    fn fills_in_defaults() {
        let host = input("example.com").build(&[]).unwrap();
        assert_eq!(host.port, 22);
        assert_eq!(host.user, default_user());
        assert_eq!(host.name, host.address());
        assert_eq!((host.group, host.note, host.identity_file), (None, None, None));
    }

    #[test]
    fn splits_a_pasted_target_into_fields() {
        let mut form = input("ssh -p 2222 deploy@example.com");
        assert!(form.split_address());
        assert_eq!((form.address.as_str(), form.port.as_str(), form.user.as_str()), ("example.com", "2222", "deploy"));
        assert!(!form.split_address());
    }

    #[test]
    fn split_only_overrides_what_was_written() {
        let mut form = HostInput { user: "admin".into(), ..input("example.com:2200") };
        assert!(form.split_address());
        assert_eq!((form.user.as_str(), form.port.as_str()), ("admin", "2200"));
    }

    #[test]
    fn build_splits_the_address_too() {
        let host = input("root@10.0.0.5:2200").build(&[]).unwrap();
        assert_eq!((host.user.as_str(), host.hostname.as_str(), host.port), ("root", "10.0.0.5", 2200));
    }

    #[test]
    fn reports_every_bad_field_at_once() {
        let form = HostInput {
            port: "70000".into(),
            user: "two words".into(),
            auth: AuthMethod::Key,
            ..input("")
        };
        assert_eq!(
            fields(form.build(&[])),
            vec![Field::Address, Field::Port, Field::User, Field::IdentityFile]
        );
        assert_eq!(fields(HostInput { port: "0".into(), ..input("a") }.build(&[])), vec![Field::Port]);
        assert_eq!(fields(input("bad host").build(&[])), vec![Field::Address]);
    }

    #[test]
    fn rejects_duplicate_names() {
        let taken = vec!["prod".to_string()];
        let form = HostInput { name: "prod".into(), ..input("example.com") };
        assert_eq!(fields(form.build(&taken)), vec![Field::Name]);
        assert!(form.build(&[]).is_ok());
    }

    #[test]
    fn checks_that_the_key_file_exists() {
        let missing = HostInput { identity_file: "/no/such/nexsh-key".into(), ..input("a") };
        assert_eq!(fields(missing.build(&[])), vec![Field::IdentityFile]);

        let path = std::env::temp_dir().join(format!("nexsh-draft-key-{}", std::process::id()));
        std::fs::write(&path, "key").unwrap();
        let form = HostInput {
            identity_file: path.display().to_string(),
            auth: AuthMethod::Key,
            ..input("a")
        };
        assert_eq!(form.build(&[]).unwrap().identity_file, Some(path.clone()));
        // 密码认证不保留私钥路径。
        let form = HostInput { auth: AuthMethod::Password, ..form };
        assert_eq!(form.build(&[]).unwrap().identity_file, None);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn round_trips_through_a_host() {
        let form = HostInput {
            port: "2222".into(),
            user: "deploy".into(),
            name: "prod".into(),
            group: "生产环境".into(),
            note: "主库".into(),
            ..input("example.com")
        };
        let host = form.build(&[]).unwrap();
        assert_eq!(HostInput::from_host(&host), form);
    }
}
