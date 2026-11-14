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
| `nexsh-mobile` | 手机端的会话引擎和 JNI 接口，编译成 Android 加载的动态库 | jni |

手机端的界面在 `mobile/android`（Kotlin + Compose），SSH、终端模拟、主机列表的读写和校验都复用上面的 Rust 核心。

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

## 手机端（Android，试验版）

主机列表（搜索、分组、长按编辑或删除）、添加主机、SSH 终端。终端上方显示连接状态和延迟，
下方是一排辅助键：Esc、Tab、方向键、Home/End、翻页、常用符号和粘贴；Ctrl 和 Alt 是粘滞键，
点一下只对下一个键生效，快速连点两下锁定。在终端里上下拖动翻看历史，双指缩放字号。
终端字体是随应用带的 JetBrains Mono（SIL OFL 1.1，许可证在 `mobile/android/app/src/main/assets/fonts/OFL.txt`），不受手机主题换字体的影响。
返回主机列表不会断开会话，列表顶部可以回到打开着的会话。

登录方式可以选密码或私钥。私钥放在手机上的私钥库里：从剪贴板粘贴或选择文件导入（OpenSSH、PEM、PuTTY 格式），
起个名字保存，之后在主机表单里从列表中选择——一把私钥导入一次，多台主机都可以用。导入时显示算法和指纹供核对；
有口令保护的私钥可以在保存时填口令，也可以留到连接时再输入。密码、私钥、口令都用系统密钥库加密后保存在手机上，
私钥不会写成明文文件。

连上之后如果断线（换网络、信号不好、服务器重启），会话自动重连：间隔从 1 秒起逐次加倍，最长 30 秒，
网络恢复或应用回到前台时立刻再试；顶部的提示条可以"立即重连"或"停止"。连续约 15 秒收不到服务器回应就判定为断线，
不必等系统超时。重连得到的是一个新的 shell，之前屏幕上的内容保留在历史里。第一次就连不上的主机不会自动重试。

长按终端里的文字选中一个词，两端出现圆点手柄，拖动它们调整范围，拖到上下边缘时画面跟着滚；
顶部的操作条可以复制、全选（包括滚出屏幕的历史）或取消。选区跟着文字走，新的输出不会让它错位。

有会话开着时应用会运行一个前台服务，通知栏里常驻一条"N 个会话保持连接中"的通知：退到后台、锁屏之后连接不断。
通知上的"全部断开"关掉所有会话并结束服务。系统的省电策略仍可能在长时间待机后掐断网络，那时回到应用会自动重连。

发布的安装包用固定密钥签名（密钥放在仓库的 Secret `ANDROID_KEYSTORE_BASE64` 里），新版本可以直接覆盖安装。
没有配置这个 Secret 时退回临时的调试签名，那样的包升级前需要先卸载旧版。

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
4. 手机端：和桌面端配对同步主机；文件浏览（等 SFTP）。
