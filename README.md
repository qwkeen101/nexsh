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
在搜索框里用上下键选择主机，回车打开。分组名里的 `/` 表示层级，例如 `生产/Web`。
右键主机（或点行尾的"更多"按钮）可以连接、复制地址、编辑、删除；右键分组可以在其中新建主机。
底部状态栏显示当前会话的状态、平滑后的往返延迟和终端尺寸。

添加或编辑主机的对话框里可以填地址、端口、用户名、名称、分组、备注，并选择认证方式：

- 自动：依次尝试 ssh-agent、`~/.ssh` 下的默认私钥（`id_ed25519`、`id_ecdsa`、`id_rsa`），都不行再询问密码。
- 密码：只用密码。
- 私钥：只用指定的私钥文件。

往地址栏粘贴 `user@host:端口` 或一条 ssh 命令会自动拆到各个字段。「测试连接」用当前填写的内容
登录一次，不保存主机，也不改动 `known_hosts`。

在对话框里填的密码和私钥口令保存在系统钥匙串里（macOS 钥匙串、Windows 凭据管理器、
Linux 的 Secret Service），配置文件里没有任何秘密；删除主机时一并清除。没有保存密码的主机，
连接时在标签页顶部询问，输入的内容只留在内存里。连接断开后按回车重新连接。
主机密钥按 `~/.ssh/known_hosts` 校验：已知主机直接连接，未知主机首次连接时记住，
密钥发生变化则拒绝连接。

## 目前的限制

- Linux 上保存密码需要正在运行的 Secret Service（GNOME Keyring、KWallet 等）；没有时密码只能每次输入。
- Windows 上尚未接入 ssh-agent。
- 不支持跳板机（ProxyJump）、主机证书，`~/.ssh/config` 的 `Include` 和 `Match` 被忽略。
- 终端还没有鼠标选择和复制；粘贴用 Cmd+V（macOS）或 Ctrl+Shift+V。
- 没有 SFTP 和端口转发。

## 路线

1. 选择与复制。
2. SFTP 和端口转发。
3. 远端 agent：会话保存在服务器上，断线重连后恢复画面。
4. 手机端，复用 Rust 核心。
