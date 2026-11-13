//! 密码和私钥口令保存在系统钥匙串里：macOS 钥匙串、Windows 凭据管理器、
//! Linux 的 Secret Service。配置文件里从不出现它们。
//!
//! 条目按主机名称命名，而不是按 `user@host:port`：两台指向同一地址的主机
//! 各有各的密码，改其中一台不会悄悄改掉另一台。

const SERVICE: &str = "nexsh";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretKind {
    Password,
    Passphrase,
}

impl SecretKind {
    pub const ALL: [SecretKind; 2] = [SecretKind::Password, SecretKind::Passphrase];
}

#[derive(Debug, thiserror::Error)]
#[error("系统钥匙串不可用: {0}")]
pub struct SecretError(String);

fn account(kind: SecretKind, host_name: &str) -> String {
    let prefix = match kind {
        SecretKind::Password => "password",
        SecretKind::Passphrase => "passphrase",
    };
    format!("{prefix}:{host_name}")
}

fn entry(kind: SecretKind, host_name: &str) -> Result<keyring::Entry, SecretError> {
    keyring::Entry::new(SERVICE, &account(kind, host_name)).map_err(|e| SecretError(e.to_string()))
}

/// 读取保存的秘密；没有保存过时为 None。
pub fn load(kind: SecretKind, host_name: &str) -> Result<Option<String>, SecretError> {
    match entry(kind, host_name)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(SecretError(error.to_string())),
    }
}

pub fn store(kind: SecretKind, host_name: &str, secret: &str) -> Result<(), SecretError> {
    entry(kind, host_name)?.set_password(secret).map_err(|e| SecretError(e.to_string()))
}

/// 删除保存的秘密；本来就没有也算成功。
pub fn delete(kind: SecretKind, host_name: &str) -> Result<(), SecretError> {
    match entry(kind, host_name)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(SecretError(error.to_string())),
    }
}

/// 主机改名后，把它的秘密挪到新名称下。
pub fn rename(old_name: &str, new_name: &str) -> Result<(), SecretError> {
    for kind in SecretKind::ALL {
        if let Some(secret) = load(kind, old_name)? {
            store(kind, new_name, &secret)?;
            delete(kind, old_name)?;
        }
    }
    Ok(())
}

/// 删除主机时清掉它的全部秘密。
pub fn forget(host_name: &str) -> Result<(), SecretError> {
    for kind in SecretKind::ALL {
        delete(kind, host_name)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_are_distinct_per_kind_and_host() {
        assert_eq!(account(SecretKind::Password, "prod"), "password:prod");
        assert_eq!(account(SecretKind::Passphrase, "prod"), "passphrase:prod");
        assert_ne!(account(SecretKind::Password, "a"), account(SecretKind::Password, "b"));
    }
}
