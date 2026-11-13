//! 手机端的主机列表：读写和桌面端同一种 `hosts.toml`，和界面之间用 JSON 交换。
//! 校验规则不在这里重写，直接用核心库的 [`HostInput`]。

use std::path::Path;

use nexsh_core::{AuthMethod, Field, Host, HostInput, HostStore};
use serde_json::{Value, json};

pub struct HostBook {
    store: HostStore,
}

fn field_name(field: Field) -> &'static str {
    match field {
        Field::Address => "address",
        Field::Port => "port",
        Field::User => "user",
        Field::Name => "name",
        Field::IdentityFile => "identityFile",
    }
}

fn auth_name(auth: AuthMethod) -> &'static str {
    match auth {
        AuthMethod::Auto => "auto",
        AuthMethod::Password => "password",
        AuthMethod::Key => "key",
    }
}

fn host_json(host: &Host) -> Value {
    json!({
        "name": host.name,
        "hostname": host.hostname,
        "port": host.port,
        "user": host.user,
        "group": host.group.clone().unwrap_or_default(),
        "note": host.note.clone().unwrap_or_default(),
        "identityFile": host.identity_file.as_ref().map(|path| path.display().to_string()).unwrap_or_default(),
        "auth": auth_name(host.auth),
        "address": host.address(),
    })
}

fn failure(errors: Vec<(&str, String)>) -> String {
    let errors: Vec<Value> = errors
        .into_iter()
        .map(|(field, message)| json!({ "field": field, "message": message }))
        .collect();
    json!({ "ok": false, "errors": errors }).to_string()
}

impl HostBook {
    pub fn open(path: &Path) -> Result<Self, String> {
        HostStore::load(path).map(|store| Self { store }).map_err(|error| error.to_string())
    }

    pub fn find(&self, name: &str) -> Option<Host> {
        self.store.find(name).cloned()
    }

    /// 全部主机，JSON 数组，顺序和文件里一致。
    pub fn list(&self) -> String {
        Value::Array(self.store.hosts().iter().map(host_json).collect()).to_string()
    }

    /// 保存表单。`original` 是被编辑的主机原来的名称，新建时为 None。
    /// 返回 `{"ok":true,"name":…}`，或 `{"ok":false,"errors":[{"field":…,"message":…}]}`；
    /// 不属于任何字段的错误（写文件失败等）的 field 是空字符串。
    pub fn save(&mut self, form: &str, original: Option<&str>) -> String {
        let Ok(form) = serde_json::from_str::<Value>(form) else {
            return failure(vec![("", "表单内容无法解析".to_string())]);
        };
        let text = |key: &str| form.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        let input = HostInput {
            address: text("address"),
            port: text("port"),
            user: text("user"),
            name: text("name"),
            group: text("group"),
            note: text("note"),
            identity_file: text("identityFile"),
            auth: match text("auth").as_str() {
                "password" => AuthMethod::Password,
                "key" => AuthMethod::Key,
                _ => AuthMethod::Auto,
            },
        };
        // 桌面端用户名留空时用本机的登录名；手机上没有这样一个名字，必须填。
        let mut probe = input.clone();
        probe.split_address();
        if probe.user.trim().is_empty() {
            return failure(vec![("user", "请填写用户名".to_string())]);
        }

        let taken: Vec<String> = self
            .store
            .hosts()
            .iter()
            .map(|host| host.name.clone())
            .filter(|name| Some(name.as_str()) != original)
            .collect();
        let host = match input.build(&taken) {
            Ok(host) => host,
            Err(errors) => {
                return failure(
                    errors.into_iter().map(|e| (field_name(e.field), e.message)).collect(),
                );
            }
        };
        let name = host.name.clone();
        let listed = match original {
            Some(original) => self.store.replace(original, host),
            None => self.store.add(host),
        };
        match listed.and_then(|_| self.store.save()) {
            Ok(()) => json!({ "ok": true, "name": name }).to_string(),
            Err(error) => failure(vec![("", error.to_string())]),
        }
    }

    pub fn remove(&mut self, name: &str) -> Result<(), String> {
        self.store.remove(name).and_then(|_| self.store.save()).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(tag: &str) -> HostBook {
        let dir = std::env::temp_dir().join(format!("nexsh-mobile-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        HostBook::open(&dir.join("hosts.toml")).unwrap()
    }

    #[test]
    fn saves_lists_and_removes() {
        let mut book = book("roundtrip");
        let form = r#"{"address":"deploy@example.com:2222","name":"prod","group":"生产"}"#;
        let reply: Value = serde_json::from_str(&book.save(form, None)).unwrap();
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["name"], "prod");

        let listed: Value = serde_json::from_str(&book.list()).unwrap();
        assert_eq!(listed[0]["hostname"], "example.com");
        assert_eq!(listed[0]["port"], 2222);
        assert_eq!(listed[0]["user"], "deploy");
        assert_eq!(listed[0]["address"], "deploy@example.com:2222");

        book.remove("prod").unwrap();
        assert_eq!(book.list(), "[]");
    }

    #[test]
    fn reports_errors_by_field() {
        let mut book = book("errors");
        let reply: Value =
            serde_json::from_str(&book.save(r#"{"address":"example.com"}"#, None)).unwrap();
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["errors"][0]["field"], "user");

        let reply: Value =
            serde_json::from_str(&book.save(r#"{"address":"","user":"root","port":"99999"}"#, None))
                .unwrap();
        let fields: Vec<&str> =
            reply["errors"].as_array().unwrap().iter().map(|e| e["field"].as_str().unwrap()).collect();
        assert_eq!(fields, ["address", "port"]);
    }
}
