//! JNI 入口，对应 Kotlin 的 `dev.nexsh.mobile.Bridge`。这里只做类型转换，逻辑都在引擎里。

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jbyteArray, jint, jlong, jstring};
use nexsh_term::KeyMods;

use crate::engine::{Engine, Session};
use crate::hosts::HostBook;

struct App {
    engine: Engine,
    book: Mutex<HostBook>,
}

static APP: OnceLock<App> = OnceLock::new();

fn book() -> Option<MutexGuard<'static, HostBook>> {
    APP.get().map(|app| app.book.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
}

fn session(id: jlong) -> Option<std::sync::Arc<Session>> {
    APP.get()?.engine.session(id as u64)
}

/// Java 字符串转成 Rust 字符串；null 得到 None。
fn string(env: &mut JNIEnv, value: &JString) -> Option<String> {
    if value.as_raw().is_null() {
        return None;
    }
    env.get_string(value).ok().map(Into::into)
}

fn java_string(env: &mut JNIEnv, text: &str) -> jstring {
    env.new_string(text).map(|value| value.into_raw()).unwrap_or(std::ptr::null_mut())
}

/// 修饰键位：1 Ctrl，2 Alt，4 Shift。
fn mods(bits: jint) -> KeyMods {
    KeyMods { ctrl: bits & 1 != 0, alt: bits & 2 != 0, shift: bits & 4 != 0 }
}

/// 应用启动时调用一次，`dir` 是应用私有的文件目录。返回空字符串表示成功，否则是错误信息。
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_start<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    dir: JString<'local>,
) -> jstring {
    let Some(dir) = string(&mut env, &dir).map(PathBuf::from) else {
        return java_string(&mut env, "没有收到数据目录");
    };
    if APP.get().is_some() {
        return java_string(&mut env, "");
    }
    let message = match HostBook::open(&dir.join("hosts.toml")) {
        Ok(book) => {
            let engine = Engine::new(dir.join("known_hosts"));
            let _ = APP.set(App { engine, book: Mutex::new(book) });
            String::new()
        }
        Err(error) => error,
    };
    java_string(&mut env, &message)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_hosts<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let list = book().map(|book| book.list()).unwrap_or_else(|| "[]".to_string());
    java_string(&mut env, &list)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_saveHost<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    form: JString<'local>,
    original: JString<'local>,
) -> jstring {
    let form = string(&mut env, &form).unwrap_or_default();
    let original = string(&mut env, &original);
    let reply = match book() {
        Some(mut book) => book.save(&form, original.as_deref()),
        None => r#"{"ok":false,"errors":[{"field":"","message":"主机列表不可用"}]}"#.to_string(),
    };
    java_string(&mut env, &reply)
}

/// 返回空字符串表示成功，否则是错误信息。
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_deleteHost<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    name: JString<'local>,
) -> jstring {
    let name = string(&mut env, &name).unwrap_or_default();
    let message = match book() {
        Some(mut book) => book.remove(&name).err().unwrap_or_default(),
        None => "主机列表不可用".to_string(),
    };
    java_string(&mut env, &message)
}

/// 打开到已保存主机的会话，返回会话编号；找不到这台主机时返回 0。
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_open<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    name: JString<'local>,
    cols: jint,
    rows: jint,
    password: JString<'local>,
    passphrase: JString<'local>,
) -> jlong {
    let name = string(&mut env, &name).unwrap_or_default();
    let password = string(&mut env, &password);
    let passphrase = string(&mut env, &passphrase);
    let Some(app) = APP.get() else { return 0 };
    let Some(host) = book().and_then(|book| book.find(&name)) else { return 0 };
    app.engine.open(host, cols.max(1) as usize, rows.max(1) as usize, password, passphrase) as jlong
}

/// 重新连接：断线之后，或者补上密码、口令之后。
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_reconnect<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: jlong,
    password: JString<'local>,
    passphrase: JString<'local>,
) {
    let password = string(&mut env, &password);
    let passphrase = string(&mut env, &passphrase);
    if let Some(session) = session(id) {
        session.connect(password, passphrase);
    }
}

/// 画面自 `since` 以来没变时返回 null。格式见 `wire` 模块。
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_snapshot<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: jlong,
    since: jlong,
) -> jbyteArray {
    session(id)
        .and_then(|session| session.snapshot(since as u64))
        .and_then(|bytes| env.byte_array_from_slice(&bytes).ok())
        .map(|array| array.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// 功能键。返回 false 表示这个名字不是功能键。
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_key<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: jlong,
    name: JString<'local>,
    modifiers: jint,
) -> jboolean {
    let name = string(&mut env, &name).unwrap_or_default();
    session(id).is_some_and(|session| session.key(&name, mods(modifiers))) as jboolean
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_text<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: jlong,
    text: JString<'local>,
    modifiers: jint,
) {
    let text = string(&mut env, &text).unwrap_or_default();
    if let Some(session) = session(id).filter(|_| !text.is_empty()) {
        session.text(&text, mods(modifiers));
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_paste<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: jlong,
    text: JString<'local>,
) {
    let text = string(&mut env, &text).unwrap_or_default();
    if let Some(session) = session(id).filter(|_| !text.is_empty()) {
        session.paste(&text);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_resize<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: jlong,
    cols: jint,
    rows: jint,
) {
    if let Some(session) = session(id) {
        session.resize(cols.max(1) as usize, rows.max(1) as usize);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_scroll<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: jlong,
    lines: jint,
) {
    if let Some(session) = session(id) {
        session.scroll(lines);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_close<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: jlong,
) {
    if let Some(app) = APP.get() {
        app.engine.close(id as u64);
    }
}
