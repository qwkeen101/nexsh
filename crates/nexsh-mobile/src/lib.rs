//! 手机端和 Rust 核心之间的桥。
//!
//! 分两层：`engine`、`hosts`、`wire` 是与平台无关的会话引擎、主机管理和帧编码，
//! 在任何平台上都能编译和测试；`android` 只是把它们接到 JNI 上的一层薄壳。
//! 手机界面（Kotlin）不解析终端序列、不碰 SSH：它每一帧来取一份画面快照，把按键和文字交回来。

pub mod engine;
pub mod hosts;
pub mod wire;

#[cfg(target_os = "android")]
mod android;
