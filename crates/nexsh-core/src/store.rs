use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::host::Host;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("找不到用户配置目录")]
    NoConfigDir,
    #[error("读写 {} 失败: {source}", .path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("{} 格式错误: {message}", .path.display())]
    Parse { path: PathBuf, message: String },
    #[error("已存在名为 {0} 的主机")]
    Duplicate(String),
    #[error("没有名为 {0} 的主机")]
    NotFound(String),
}

#[derive(Default, Serialize, Deserialize)]
struct HostsFile {
    #[serde(default, rename = "host")]
    hosts: Vec<Host>,
}

/// 保存在 `hosts.toml` 里的主机列表。
#[derive(Debug)]
pub struct HostStore {
    path: PathBuf,
    hosts: Vec<Host>,
}

impl HostStore {
    /// 默认位置：系统配置目录下的 `nexsh/hosts.toml`。
    pub fn default_path() -> Result<PathBuf, StoreError> {
        let dirs =
            directories::ProjectDirs::from("", "", "nexsh").ok_or(StoreError::NoConfigDir)?;
        Ok(dirs.config_dir().join("hosts.toml"))
    }

    pub fn load_default() -> Result<Self, StoreError> {
        Self::load(Self::default_path()?)
    }

    /// 文件不存在时得到空列表。
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let hosts = match std::fs::read_to_string(&path) {
            Ok(text) => {
                toml::from_str::<HostsFile>(&text)
                    .map_err(|e| StoreError::Parse { path: path.clone(), message: e.to_string() })?
                    .hosts
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(source) => return Err(StoreError::Io { path, source }),
        };
        Ok(Self { path, hosts })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn hosts(&self) -> &[Host] {
        &self.hosts
    }

    pub fn find(&self, name: &str) -> Option<&Host> {
        self.hosts.iter().find(|h| h.name == name)
    }

    pub fn add(&mut self, host: Host) -> Result<(), StoreError> {
        if self.find(&host.name).is_some() {
            return Err(StoreError::Duplicate(host.name));
        }
        self.hosts.push(host);
        Ok(())
    }

    /// 用 `host` 替换名为 `name` 的主机，位置不变；改名时不能和别的主机重名。
    pub fn replace(&mut self, name: &str, host: Host) -> Result<(), StoreError> {
        let index = self
            .hosts
            .iter()
            .position(|h| h.name == name)
            .ok_or_else(|| StoreError::NotFound(name.to_string()))?;
        if host.name != name && self.find(&host.name).is_some() {
            return Err(StoreError::Duplicate(host.name));
        }
        self.hosts[index] = host;
        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> Result<Host, StoreError> {
        let index = self
            .hosts
            .iter()
            .position(|h| h.name == name)
            .ok_or_else(|| StoreError::NotFound(name.to_string()))?;
        Ok(self.hosts.remove(index))
    }

    /// 先写临时文件再改名，避免写到一半时崩溃留下损坏的配置。
    pub fn save(&self) -> Result<(), StoreError> {
        let io_err = |source| StoreError::Io { path: self.path.clone(), source };
        let text = toml::to_string_pretty(&HostsFile { hosts: self.hosts.clone() })
            .map_err(|e| StoreError::Parse { path: self.path.clone(), message: e.to_string() })?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(io_err)?;
        }
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(io_err)?;
        std::fs::rename(&tmp, &self.path).map_err(io_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("nexsh-store-{tag}-{}", std::process::id())).join("hosts.toml")
    }

    #[test]
    fn round_trips_hosts() {
        let path = temp_path("roundtrip");
        let mut store = HostStore::load(&path).unwrap();
        assert!(store.hosts().is_empty());

        let mut host = Host::parse_target("deploy@example.com:2222").unwrap();
        host.name = "prod".into();
        host.identity_file = Some("/keys/prod".into());
        store.add(host.clone()).unwrap();
        assert!(matches!(store.add(host.clone()), Err(StoreError::Duplicate(_))));
        store.save().unwrap();

        let reloaded = HostStore::load(&path).unwrap();
        assert_eq!(reloaded.hosts(), &[host]);

        let mut reloaded = reloaded;
        let mut renamed = host.clone();
        renamed.name = "production".into();
        assert!(matches!(reloaded.replace("missing", renamed.clone()), Err(StoreError::NotFound(_))));
        reloaded.replace("prod", renamed.clone()).unwrap();
        assert_eq!(reloaded.hosts(), &[renamed]);
        reloaded.replace("production", host.clone()).unwrap();
        reloaded.remove("prod").unwrap();
        assert!(matches!(reloaded.remove("prod"), Err(StoreError::NotFound(_))));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn reports_malformed_file() {
        let path = temp_path("malformed");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[[host]]\nname = 3\n").unwrap();
        assert!(matches!(HostStore::load(&path), Err(StoreError::Parse { .. })));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
