# xattr 0.2.3（本地修补版）

这不是 nexsh 自己的代码，是第三方库 [xattr](https://github.com/Stebalien/xattr) 0.2.3 的副本，
许可证为 MIT / Apache-2.0（见同目录的 LICENSE 文件）。

## 为什么放在这里

GPUI 0.2.2 通过 zed-async-tar 间接依赖 xattr 0.2.3。这个旧版本在 Linux 上引用
`libc::ENOATTR`，而新版 libc 已经在 Linux 上去掉了这个常量（Linux 用的是 `ENODATA`），
导致界面在 Linux 上编译不过。

## 改了什么

只改了 `src/sys/mod.rs`：Linux 和 Android 上用 `libc::ENODATA` 代替 `libc::ENOATTR`，
其余平台不变。根目录 `Cargo.toml` 里的 `[patch.crates-io]` 让构建使用这份副本。

## 什么时候可以删

GPUI 升级到不再依赖 xattr 0.2.x 的版本后，删掉这个目录和 `[patch.crates-io]` 里对应的一行。
