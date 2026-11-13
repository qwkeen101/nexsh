//! JNI 入口，对应 Kotlin 的 `dev.nexsh.mobile.Bridge`。这里只做类型转换，逻辑都在引擎里。

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jbyteArray, jint, jlong, jstring};
use nexsh_term::{KeyMods, SelectionEnd};

use crate::engine::{Credentials, Engine, Session};
use crate::hosts::{HostBook, describe_key};

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

/// 检查一段文字是不是私钥。返回 JSON，见 `hosts::describe_key`。
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_inspectKey<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    text: JString<'local>,
) -> jstring {
    let text = string(&mut env, &text).unwrap_or_default();
    java_string(&mut env, &describe_key(&text))
}

/// 打开到已保存主机的会话，返回会话编号；找不到这台主机时返回 0。
/// 密码、口令、私钥内容没有就传 null；它们只留在这个会话的内存里，供重连时使用。
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_open<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    name: JString<'local>,
    cols: jint,
    rows: jint,
    password: JString<'local>,
    passphrase: JString<'local>,
    key: JString<'local>,
) -> jlong {
    let name = string(&mut env, &name).unwrap_or_default();
    let credentials = Credentials {
        password: string(&mut env, &password),
        passphrase: string(&mut env, &passphrase),
        key: string(&mut env, &key),
    };
    let Some(app) = APP.get() else { return 0 };
    let Some(host) = book().and_then(|book| book.find(&name)) else { return 0 };
    app.engine.open(host, cols.max(1) as usize, rows.max(1) as usize, credentials) as jlong
}

/// 重新连接：断线之后，或者补上密码、口令之后。传 null 的沿用会话记着的。
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

/// 正在等待重连的会话马上重连，不等退避时间到。
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_retryNow<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: jlong,
) {
    if let Some(session) = session(id) {
        session.retry_now();
    }
}

/// 停止自动重连，会话停在"已断开"。
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_stopRetry<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: jlong,
) {
    if let Some(session) = session(id) {
        session.stop_retry();
    }
}

/// 网络恢复或应用回到前台：所有等着重连的会话都马上试一次。
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_networkAvailable<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) {
    if let Some(app) = APP.get() {
        app.engine.retry_all();
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

/// 选中可见区域里这个位置上的词。
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_selectWord<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: jlong,
    row: jint,
    col: jint,
) {
    if let Some(session) = session(id) {
        session.select_word(row.max(0) as usize, col.max(0) as usize);
    }
}

/// 把选区的一端拖到这个位置；end 为 0 是起点，其余是终点。
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_moveSelection<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: jlong,
    end: jint,
    row: jint,
    col: jint,
) {
    let end = if end == 0 { SelectionEnd::Start } else { SelectionEnd::End };
    if let Some(session) = session(id) {
        session.move_selection(end, row.max(0) as usize, col.max(0) as usize);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_selectAll<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: jlong,
) {
    if let Some(session) = session(id) {
        session.select_all();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_clearSelection<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: jlong,
) {
    if let Some(session) = session(id) {
        session.clear_selection();
    }
}

/// 选中的文字；没有选区时返回 null。
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_nexsh_mobile_Bridge_selectionText<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: jlong,
) -> jstring {
    match session(id).and_then(|session| session.selection_text()) {
        Some(text) => java_string(&mut env, &text),
        None => std::ptr::null_mut(),
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
