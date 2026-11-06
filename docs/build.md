# 构建与验证

## Windows（本仓库实测平台）

需要 **Rust stable GNU target** + 完整的 **MinGW-w64 binutils**（`dlltool`、`as`）。
不需要 OpenSSL，也不需要 MSVC。

> 只靠 rustup 自带的 `self-contained\dlltool.exe` 不够：它找不到 `as` 时只能生成空导入库，
> `windows-sys` / `parking_lot_core` 会编译失败。

仓库**不再**钉死 `rust-toolchain.toml`（否则非 Windows 用户 clone 后 `cargo build` 会直接报错）。
Windows 本地构建用一次性的目录级 override：

```powershell
rustup override set stable-x86_64-pc-windows-gnu
cargo build --release
```

PATH 需要包含：

- `%USERPROFILE%\.cargo\bin`
- WinLibs 的 `mingw64\bin`（例如
  `%LOCALAPPDATA%\Microsoft\WinGet\Packages\BrechtSanders.WinLibs.POSIX.UCRT_*\mingw64\bin`）

如果当前终端是旧会话，PATH 可能还是旧的，重启终端即可。不要显式使用
`--target x86_64-pc-windows-msvc`——该 target 未安装。

产物：`target\release\search-gateway.exe`。

## Linux / macOS（未实测）

用本机 stable 覆盖即可：

```bash
rustup override set stable      # 或每次 cargo +stable build --release
cargo build --release
```

CI 会为 `x86_64-unknown-linux-gnu`、`aarch64-unknown-linux-gnu`、`x86_64-apple-darwin`、
`aarch64-apple-darwin` 构建产物，但**运行未经过手动验证**。

## 依赖选择

优先 `rustls` + `ring`，避免 `openssl-sys` / `native-tls` 这类需要额外本地库的 crate。
`ring` 已在 GNU target 下验证可构建。

## 验证

```bash
# 单元测试 + wiremock 集成测试 + MCP stdio 冒烟测试（46 + 29 + 2 = 77）
cargo test

# 10 条 golden query，走 SearXNG，零 API 费用
cargo run -- --selftest --config tests/fixtures/live-config.toml

# 配置、keyring、熔断器、SearXNG 检查（不消耗付费额度）
cargo run -- --doctor --config tests/fixtures/live-config.toml

# 加上 Tavily/Firecrawl 的真实探测（各消耗一个请求）+ 一次免费抓取探测
cargo run -- --doctor --live --config tests/fixtures/live-providers.toml

# golden set 走完整付费链
cargo run -- --selftest --live-providers --config tests/fixtures/live-providers.toml
```

`tests/fixtures/live-providers.toml` 从环境变量读取 key，不含任何密钥。

- `--selftest` 的通过条件：每条查询至少 3 条来源，且 p95 延迟低于 8000ms。
- `--doctor` 不带 `--live` 时付费探测被跳过，整体状态为 `unknown`、`live_verified=false`；
  只有 `misconfigured` 或 `unreachable` 会让 CLI 退出码非零。
- `--doctor --live` 额外抓取 `[doctor] fetch_probe_url`（免费，不走 Firecrawl）。

## 目录结构

```text
src/
  app.rs            组装配置、提供方客户端和引擎
  config.rs         配置加载与 key 解析
  keyring.rs        多 key 轮询与冷却
  circuit.rs        提供方熔断器
  providers/        Tavily、Firecrawl、SearXNG 客户端
  search.rs         搜索链、补位、兜底、缓存
  fetch/            抓取链、专用解析器、通用正文提取
  map.rs            站点 URL 发现
  normalize.rs      URL/日期归一化、去重、截断
  cache.rs          LRU 搜索缓存与来源集持久化
  rate.rs           进程级滑动窗口限流
  doctor.rs         诊断
  selftest.rs       golden set
  mcp.rs            MCP 工具定义
```

## 附：MSVC 被卸载后的影响

- 已在运行的预编译二进制（例如 npm 分发的可执行文件）不受影响。
- `vcruntime140.dll`、`vcruntime140_1.dll`、`msvcp140.dll` 仍在 `System32`，
  预编译 MSVC 程序可继续运行。
