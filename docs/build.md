# 构建环境

本项目使用 Rust GNU target，不依赖 MSVC 工具链。

## 工具链

- `stable-x86_64-pc-windows-gnu`（当前 rustc 1.99.1）
- 建议在仓库根放 `rust-toolchain.toml`：

```toml
[toolchain]
channel = "stable-x86_64-pc-windows-gnu"
```

## PATH

- `%USERPROFILE%\.cargo\bin`
- WinLibs：`%LOCALAPPDATA%\Microsoft\WinGet\Packages\BrechtSanders.WinLibs.POSIX.UCRT_*\mingw64\bin`

这两个路径已在持久化用户 PATH 中；如果当前终端是旧会话，可能仍是旧 PATH，重启终端或 Claude Code 即可。

## MSVC 清理的影响

- 已运行的 `grok-search-rs` 是 npm 预编译二进制，不依赖本机 MSVC 工具链。
- `vcruntime140.dll`、`vcruntime140_1.dll`、`msvcp140.dll` 仍在 System32，预编译 MSVC 程序可继续运行。
- 不要显式使用 `--target x86_64-pc-windows-msvc`；该 target 未安装。

## 依赖选择

优先 `rustls` + `ring`，避免 `openssl-sys` / `native-tls` 这类需要额外本地库的 crate。`ring` 已在 GNU target 下验证可构建。
