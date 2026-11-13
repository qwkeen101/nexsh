//! nexsh 的核心库：主机配置、SSH 连接与会话。不依赖任何界面框架。

pub mod host;
pub mod latency;
pub mod search;
pub mod ssh;
pub mod ssh_config;
pub mod store;

pub use host::Host;
pub use latency::{LatencyLevel, LatencyMeter};
pub use ssh::{
    ConnectOptions, HostKeyPolicy, SessionCommand, SessionEvent, SessionHandle, SshError,
    spawn_session,
};
pub use store::HostStore;

use std::sync::OnceLock;
use tokio::runtime::Runtime;

/// 全进程共用的 tokio 运行时。界面线程不跑 tokio，所有网络 IO 都在这里。
pub fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("nexsh-io")
            .enable_all()
            .build()
            .expect("无法创建 tokio 运行时")
    })
}
