# nexsh

用 Rust 编写、由 GPU 渲染界面的原生跨平台 SSH 客户端。

当前是第一阶段的骨架：主机列表、多标签终端、普通 SSH 连接。

## 结构

| crate | 作用 | 主要依赖 |
|---|---|---|
| `nexsh-core` | 主机配置、SSH 连接与会话，不依赖界面 | russh、tokio |
| `nexsh-term` | 终端模拟（VT 解析、字符网格、按键编码） | alacritty_terminal |
| `nexsh-cli` | 命令行工具，用来管理主机和验证 SSH 实现 | crossterm |
| `nexsh-app` | 桌面界面，可执行文件名为 `nexsh` | gpui |

## 构建

需要 Rust 1.89 或更新版本。

```sh
# 核心库、终端库、命令行工具，以及它们的测试
cargo test

# 桌面界面
cargo run --release -p nexsh-app
```

界面在 Linux 上还需要系统库，Debian/Ubuntu：

```sh
sudo apt-get install -y build-essential clang cmake pkg-config \
  libasound2-dev libfontconfig-dev libssl-dev libzstd-dev \
  libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libx11-xcb-dev libxcb1-dev libvulkan1
```

macOS 需要 Xcode（含 Metal 工具链），Windows 需要 MSVC 构建工具。

推送到 GitHub 后，`.github/workflows/ci.yml` 会在三个平台上跑测试并编译界面，
可执行文件在 Actions 页面的 Artifacts 里下载。

## 使用

主机来自两处：自己保存的列表（界面侧栏的「+ 添加」，或 `nexsh-cli host add`），以及 `~/.ssh/config` 里的具体别名。

```sh
nexsh-cli host add prod deploy@example.com:2222 -i ~/.ssh/prod_key
nexsh-cli host list
nexsh-cli connect prod          # 在当前终端里直接连接
nexsh-cli connect root@10.0.0.5 # 也可以不保存直接连
```

启动 `nexsh` 后在左侧点击主机即可打开终端标签页。侧栏顶部的搜索框按名称、地址、分组筛选主机，
也可以直接输入 `user@host:端口` 或一条 `ssh -p 2222 user@host` 命令，回车连接而不保存。
底部状态栏显示当前会话的状态、平滑后的往返延迟和终端尺寸。

需要密码或私钥口令时，标签页顶部会出现输入框。
密码和口令只保存在内存里，用于该标签页重连，不写入任何文件。连接断开后按回车重新连接。

认证顺序：ssh-agent、私钥文件（主机指定的，或 `~/.ssh/id_ed25519`、`id_ecdsa`、`id_rsa`）、密码。
主机密钥按 `~/.ssh/known_hosts` 校验：已知主机直接连接，未知主机首次连接时记住，
密钥发生变化则拒绝连接。

## 目前的限制

- 界面里还不能编辑已保存的主机（可以删除后重新添加）。
- Windows 上尚未接入 ssh-agent。
- 不支持跳板机（ProxyJump）、主机证书，`~/.ssh/config` 的 `Include` 和 `Match` 被忽略。
- 终端还没有鼠标选择和复制；粘贴用 Cmd+V（macOS）或 Ctrl+Shift+V。
- 没有 SFTP 和端口转发。

## 路线

1. 主机编辑、选择与复制。
2. SFTP 和端口转发。
3. 远端 agent：会话保存在服务器上，断线重连后恢复画面。
4. 手机端，复用 Rust 核心。
