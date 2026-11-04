# search-gateway

只做检索的 MCP 服务：把 Tavily、Firecrawl、SearXNG 的结果归一化成“来源”返回给宿主模型，由宿主模型自己写答案和引用。

- 不生成答案，不做摘要，不判断“哪个来源更对”。
- 搜索链：Tavily → Firecrawl（补位/故障转移）→ SearXNG（前两者全部失败或零结果时兜底）。
- 抓取链：站点专用解析器 → 通用正文提取 → Firecrawl。
- 多 key 轮询、按错误类型冷却、按操作熔断（搜索/抓取/地图互不阻塞）、共享时间预算、内存 LRU 搜索缓存（来源集可持久化）、全局调用速率限制。
- 当前版本 0.2.0；面向 agent 的 `isError`、截断与缓存新鲜度契约见 [`docs/adr/0005-agent-response-contract.md`](docs/adr/0005-agent-response-contract.md)。

设计背景见 [`CONTEXT.md`](CONTEXT.md)、[`docs/`](docs/) 和 [`docs/adr/`](docs/adr/)。

## 工具

| 工具 | 作用 | 主要参数 |
| --- | --- | --- |
| `web_search` | 搜索并返回归一化来源 | `query`、`max_results` 1–50（默认 5）、`recency_days` 1–365、`snippet_max_chars` 100–2000（默认 800）、`fresh`（true 跳过查询缓存）、`include_domains`/`exclude_domains` ≤10、`categories`（`general`/`it`/`science`，仅 SearXNG 兜底生效；省略时使用 `[searxng] categories`） |
| `web_fetch` | 抓取单个 URL，返回 Markdown；长文可分页续读 | `url`、`max_chars` 1–200000（默认 12000）、`offset`、`fetch_id`（`offset > 0` 时必须提供） |
| `web_map` | 发现站点 URL | `url`、`max_results` 1–200；响应受 `[map] response_max_chars` 约束 |
| `get_sources` | 按 `search_id` 分页读取上次搜索的完整来源集，不重新搜索 | `search_id`、`offset`、`limit` 1–50、`snippet_max_chars` 100–2000（默认沿用原搜索的 cap） |
| `doctor` | 检查配置、keyring、熔断器、SearXNG 健康与引擎状态 | `live`（true 时向 Tavily/Firecrawl 各发一个最小请求，并对 `[doctor] fetch_probe_url` 做一次免费抓取探测） |

`web_search` 返回的 `meta.search_id` 用于 `get_sources`。搜索缓存分两层：相同参数的查询命中查询缓存（进程内 LRU，默认 600 秒、最多 32 条），以及供 `get_sources` 分页的来源集缓存（默认持久化，重启后未过期的 `search_id` 仍有效）。命中查询缓存时 `meta.cached=true`、`meta.cache_age_secs` 给出结果年龄、`meta.chain_used=["cache"]`；需要最新结果时传 `fresh=true` 绕过查询缓存。命中会复用原来的来源集 `search_id`，不会新建缓存条目；来源集被淘汰时会自动重建。空结果或带 `provider_errors` 的结果不会进入查询缓存。`meta.snippet_only=true` 表示本次返回的每条来源只有搜索摘要，引用数字或具体论断前必须先 `web_fetch`。

响应按整条来源整形：每条摘要默认最多 800 字符，`web_search` 与每个 `get_sources` 页还受整响应预算（默认 30000 字符）约束。预算从尾部移除来源，返回 `meta.truncated=true` 与 `meta.omitted_sources`；被移除的来源仍保留在缓存中，可用 `search_id` 继续分页读取。`get_sources` 默认使用原搜索的摘要 cap，可用 `snippet_max_chars` 单次覆盖；响应返回实际生效的 `snippet_max_chars`；任一摘要被 cap 截断，或预算移除本页尾部来源时 `truncated=true`（`next_offset` 指向下一批未返回的来源）。`get_sources` 同样返回 `snippet_only=true`，表示这批来源仍只有摘要。

`web_fetch` 返回完整提取文本的字符数 `original_length`、提取置信度 `quality`（`ok`/`low`/`unknown`）与 `quality_score`（0–100）、引用门 `citation_ready` 与原因 `citation_caveat`，以及 `fetch_id`、`next_offset`、`end_of_content`、`http_status`。`citation_ready=false` 表示低质量、源站 4xx、warning 或页面截断。若因截断导致，先按 `fetch_id` + `next_offset` 续读到 `end_of_content=true`；否则宿主必须在每个相关论断旁披露 `citation_caveat`，或换来源。`citation_ready=false` 的页面不得无 caveat 出现在来源列表里。正文超过 `max_chars` 时 `truncated=true`；把同一 `url`、`fetch_id` 和 `next_offset` 作为 `offset` 再调用一次即可读取下一段，续读走本地页面缓存，不发起网络请求。`offset > 0` 必须同时提供 `fetch_id`，否则在发起网络请求前返回 `invalid_request`。`fetch_id` 必须与产生它的 `url` 配对；URL 不一致时返回 `unknown_fetch_id`（消息会说明是不同 URL）。`end_of_content=true` 表示已经读到缓存正文末尾。页面缓存默认保存完整正文（最多 100 万字符），有效期由 `[cache] page_ttl_secs` 控制（默认 600 秒），容量由 `[cache] max_pages` 控制（默认 64 页）。未知或过期的 `fetch_id` 返回 `unknown_fetch_id` 且 `isError=true`。`http_status` 是最后一次向源站发起请求得到的状态码；源站返回 4xx 但仍有可用内容时，响应通过 `warning.code=http_4xx` 报告，`error=null`、`isError=false`（便于判断内容是否来自错误页）。只有 403/429 会尝试 Firecrawl 兜底；404/410 等直接返回 `http_4xx`，不消耗 Firecrawl 额度。通用提取的 `quality_score` 低于 45 时，若配置了 Firecrawl 会自动改用 Firecrawl 并取分数更高者；HTML 缺少 charset 头时会读取前 8KB 的 `<meta charset>`，评分对 CJK 字符按双宽计入，避免短中文正文被误判为低质量。

`recency_days` 上限为 365：Tavily/Firecrawl/SearXNG 的原生时间过滤只支持 day/week/month/year，因此服务端还会在归一化阶段按 `published_date` 做一次本地过滤，丢弃日期可解析且早于 `now - recency_days` 的来源。没有日期或日期无法解析的来源会保留，避免丢失大量结果；超过一年的值会被拒绝，而不是静默忽略。

`categories` 只传给 SearXNG，且 SearXNG 仅作为最后兜底；Tavily/Firecrawl 没有等价参数。省略 `categories` 时使用 `[searxng] categories`，该配置为空才回退到 `general`。SearXNG 没有原生的 include/exclude 域名参数，服务端会把它们拼成 `site:` / `-site:` 查询词，并保留本地域名后过滤作为双保险。

`web_search`、`web_fetch`、`web_map` 的返回结构里都带 `provider_errors`（`web_fetch` 为 `warning` 与 `error`），错误码包括 `unreachable`、`timeout`、`http_5xx`、`http_4xx`、`invalid_request`、`invalid_response`、`auth`、`quota_exhausted`、`rate_limited`、`all_keys_cooling`、`circuit_open`、`misconfigured`、`blocked`、`unsupported_content_type`、`zero_results`、`budget_exhausted`、`unknown_search_id`、`unknown_fetch_id`。提供方返回 `Retry-After` 时，对应错误会带 `retry_after_secs`；零来源搜索还会在 `meta.retry_after_secs` 给出各错误中的最大值。`http_4xx` 表示源站对抓取返回了 4xx；`invalid_request` 表示搜索提供方拒绝了本次请求参数，两者都不会触发提供方级熔断。

每个出站工具调用（搜索、抓取、站点地图）都经过进程级滑动窗口限流，默认每分钟 60 次，`[rate] max_calls_per_minute = 0` 可关闭。超限时返回 `provider_errors.local`（`rate_limited`，带 `retry_after_secs`），不消耗任何提供方额度。

工具级失败使用 MCP `isError=true`，规则如下：

| 工具 | `isError=true` | `isError=false` |
| --- | --- | --- |
| `web_search` | 零来源，且至少一个 `provider_errors` 不是 `zero_results`（如鉴权失败、超时、限流、预算耗尽） | 有来源；或零来源但所有错误都是 `zero_results`（干净的零结果） |
| `web_fetch` | `error` 非空（全部提取失败、URL 非法、被 SSRF 拦截、未知 `fetch_id` 等） | 返回了内容；源站 4xx 但有可用内容时只带 `warning` |
| `web_map` | 没有任何提供方成功（`provider` 为空，如未配置或全部失败） | 提供方成功，即使返回零 URL |
| `get_sources` | `search_id` 未知或过期 | 有效 `search_id`，即使页为空 |

`web_search` 只要返回了来源就保持 `isError=false`，部分提供方失败放在 `provider_errors` 中。零结果时 `web_search` 会写入 `provider_errors["chain"]`（`zero_results`）；SearXNG 兜底零结果时消息会带上不可用引擎名。`web_map` 在未配置 Tavily/Firecrawl 时写入 `provider_errors["map"]`（`misconfigured`）。

## 构建

要求 Rust stable GNU 目标（本仓库固定 `stable-x86_64-pc-windows-gnu`），并且 `PATH` 中有完整的 MinGW-w64 binutils（`dlltool`、`as`）。只靠 rustup 自带的 `self-contained\dlltool.exe` 不够：它找不到 `as` 时只能生成空导入库，`windows-sys`/`parking_lot_core` 会编译失败。无需 OpenSSL/MSVC。

```bash
cargo build           # debug 构建，当前环境已验证
cargo test
cargo build --release # 需要 MinGW-w64 binutils
```

产物：`target\debug\search-gateway.exe` 或 `target\release\search-gateway.exe`。

Linux/macOS 不要直接继承本仓库固定的 Windows GNU 工具链；用本机 stable 覆盖：
`cargo +stable build --release`。跨机器安装与验证见
[`skills/search-gateway/references/portable-setup.md`](skills/search-gateway/references/portable-setup.md)。

## 配置

配置路径按优先级：

1. `--config PATH`
2. 环境变量 `SEARCH_GATEWAY_CONFIG`
3. OS 默认路径：Windows `%APPDATA%\search-gateway\config.toml`；Linux/macOS（有 `HOME` 时）
   `~/.config/search-gateway/config.toml`
4. 仅当 `APPDATA` 和 `HOME` 都未设置时，才使用当前目录 `config.toml`
5. 所选文件不存在时，服务模式报错；`--doctor` / `--selftest` 使用内置默认值并给出警告

当前目录的 `config.toml` **不是** OS 默认路径之后的兜底。要使用仓库内或项目内的配置，
必须显式传 `--config`，或设置 `SEARCH_GATEWAY_CONFIG`。

文件不会自动创建。服务模式（不带 `--doctor`/`--selftest`）启动时若配置无效会直接报错，并把示例配置打印到错误信息里；`--doctor`/`--selftest` 使用宽松加载，仍会报告警告。

最小可用配置（SearXNG 兜底，零 API 费用）：

```toml
[searxng]
enabled = true
base_url = "http://127.0.0.1:12310"
```

完整示例（略去注释，字段与默认值一致）：

```toml
[tavily]
enabled = true
base_url = "https://api.tavily.com"
keys = ["env:TAVILY_API_KEY"]
search_depth = "basic"
timeout_ms = 10000

[firecrawl]
enabled = true
base_url = "https://api.firecrawl.dev"
keys = ["env:FIRECRAWL_API_KEY"]
timeout_ms = 10000

[searxng]
enabled = true
base_url = "http://127.0.0.1:12310"
connect_timeout_ms = 2000
request_timeout_ms = 8000
server_timeout_s = 6
categories = ["general"]
language = "auto"
max_results = 10
required_engines = ["google cse", "bing", "360search"]
disabled_engines = ["brave", "duckduckgo", "startpage"]

[search]
budget_ms = 30000
default_max_results = 5
snippet_max_chars = 800
response_max_chars = 30000
reserve_last_resort_ms = 8000

[fetch]
budget_ms = 30000
max_chars_default = 12000
max_chars_limit = 200000
specialist_timeout_ms = 10000
generic_timeout_ms = 8000
firecrawl_timeout_ms = 12000
allow_private_hosts = false

[map]
budget_ms = 20000
default_max_results = 50
max_results_limit = 200
response_max_chars = 30000

[keyring]
auth_cooldown_secs = 600
quota_cooldown_secs = 3600
rate_limit_cooldown_secs = 60
transport_retry = 1

[circuit]
failure_threshold = 3
cooldown_secs = 300
misconfig_cooldown_secs = 600

[cache]
max_sessions = 32
ttl_secs = 600
max_sources_per_session = 200
max_pages = 64
page_ttl_secs = 600
persist = true

[rate]
max_calls_per_minute = 60

[doctor]
fetch_probe_url = "https://example.com/"

[log]
level = "info"
file = false
```

### Key 解析

`keys` 的每一项可以是字面值，也可以是 `env:NAME`。一个环境变量的值还可以是用逗号、分号或空白分隔的多个 key：

```toml
[tavily]
keys = ["env:TAVILY_API_KEY"]
# TAVILY_API_KEY="tvly-aaa,tvly-bbb,tvly-ccc" 会解析成 3 个 key
```

多 key 按轮询使用；`401/403` 冷却 600 秒，`402/432/433` 冷却 3600 秒，`429` 冷却 60 秒（若响应带 `Retry-After` 则优先使用）。同一提供方的 keyring 在搜索、抓取、站点地图和 doctor 之间共享；熔断器按操作区分（`tavily.search`、`tavily.map`、`firecrawl.search`、`firecrawl.scrape`、`firecrawl.map`、`searxng.search`），一个操作失败不会阻塞另一个。所有 key 都在冷却时该提供方直接故障转移，不打开熔断器。

如果 `keys` 为空，会回退到旧环境变量 `TAVILY_API_KEY` / `FIRECRAWL_API_KEY`。key 不会出现在日志或 doctor 输出中，doctor 只报告数量和健康状态。

## 作为 MCP 服务运行

单独新增一个 MCP 条目即可，不要覆盖现有的 `grok-search-rs`：

```json
{
  "mcpServers": {
    "search-gateway": {
      "command": "D:\\tools\\search-gateway\\target\\release\\search-gateway.exe",
      "args": ["--config", "D:\\tools\\search-gateway\\config.toml"]
    }
  }
}
```

服务使用 stdio 传输；日志写 stderr，不会污染 MCP 消息流。

## Agent 集成（可移植）

仓库自带 Claude Code skill 和标准提示词，目标是在另一台电脑、本机另一个 agent 或另一个
profile 上使用同一套检索与引用标准，而不是每台机器各写一套。

| 文件 | 作用 |
| --- | --- |
| [`skills/search-gateway/SKILL.md`](skills/search-gateway/SKILL.md) | 检索/引用工作流、工具参考、错误处理 |
| [`skills/search-gateway/references/agent-prompt.md`](skills/search-gateway/references/agent-prompt.md) | 全局提示词 §1/§1b 的唯一来源 |
| [`skills/search-gateway/references/answer-quality.md`](skills/search-gateway/references/answer-quality.md) | 引用审计清单与评分标准 |
| [`skills/search-gateway/references/portable-setup.md`](skills/search-gateway/references/portable-setup.md) | 跨机器安装、验证、漂移检查 |
| [`skills/search-gateway/references/local-environment.example.md`](skills/search-gateway/references/local-environment.example.md) | 参考机器的本地部署示例，不属于标准 |
| [`skills/search-gateway/scripts/install.ps1`](skills/search-gateway/scripts/install.ps1) / [`install.sh`](skills/search-gateway/scripts/install.sh) | 把 skill 安装/覆盖到用户目录 |
| [`config.example.toml`](config.example.toml) | 无密钥配置模板 |

`references/search-routing-policy.md` 只是指向 `agent-prompt.md` 的指针，不维护第二份策略。

安装 skill：

```powershell
# Windows
.\skills\search-gateway\scripts\install.ps1 -Force
```

```bash
# Linux/macOS
./skills/search-gateway/scripts/install.sh --force
```

然后把 `agent-prompt.md` 的 §1/§1b 复制进 `~/.claude/CLAUDE.md`，按
`portable-setup.md` 注册 MCP、配置权限并运行 `doctor`。**标准层只改仓库**：路径、端口、
key、代理网段放在本地 `config.toml`，不要写进 skill 或全局提示词。

**发布组合**：release 二进制（工具描述与字段）、`~/.claude/skills/search-gateway`、
`~/.claude/CLAUDE.md` 的 §1/§1b 必须同版。改仓库后重新构建、重装 skill、同步全局提示词；
只更新一层会出现“skill 有硬门、二进制或提示词没有”的漂移。

验收按 `answer-quality.md` 的 8 项检查，不只看“有没有来源列表”。硬门是数字/日期覆盖、
URL 溯源、来源身份、抓取质量、独立性；另外检查冲突、检索记录、URL 精确性。任何一项
不过，不算 A 级。

2026-11-03 的验证复盘见
[`docs/reviews/2026-11-03-search-gateway-verification.md`](docs/reviews/2026-11-03-search-gateway-verification.md)。

## 验证

```bash
# 单元测试 + wiremock 集成测试 + MCP stdio 冒烟测试
cargo test

# 10 条 golden query，走 SearXNG，零 API 费用
cargo run -- --selftest --config tests/fixtures/live-config.toml

# 配置、keyring、熔断器、SearXNG 检查（不消耗付费额度）
cargo run -- --doctor --config tests/fixtures/live-config.toml

# 加上 Tavily/Firecrawl 的真实探测（各消耗一个请求），并做一次免费抓取探测
cargo run -- --doctor --live --config tests/fixtures/live-providers.toml

# golden set 走完整付费链
cargo run -- --selftest --live-providers --config tests/fixtures/live-providers.toml
```

`tests/fixtures/live-providers.toml` 从环境变量读取 key，不含任何密钥。`--selftest` 的通过条件是每条查询至少 3 条来源且 p95 延迟低于 8000ms。`--doctor` 不带 `--live` 时付费探测被跳过，整体状态为 `unknown`、`live_verified=false`；只有 `misconfigured` 或 `unreachable` 会让 CLI 退出码非零。带 `--live` 时额外抓取 `[doctor] fetch_probe_url`（免费，不走 Firecrawl）。

## 目录

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

## 已知边界

- 只返回来源，不生成答案；引用由宿主模型完成。
- 查询缓存（相同参数重复搜索）是进程内内存，命中时通过 `meta.cached`/`meta.cache_age_secs` 暴露年龄，`fresh=true` 可绕过；来源集缓存默认持久化到配置文件同目录的 `sessions.json`（无配置文件时为 `%APPDATA%\search-gateway\sessions.json`），因此 `get_sources` 在重启后仍可读取未过期的 `search_id`，持久化写入在后台线程完成、进程退出时可能丢失最后一次未落盘的写入；key 冷却状态和 `fetch_id` 页面缓存不持久化。
- `web_fetch` 默认拒绝 loopback、私有网段、link-local、multicast 和保留地址；确需抓本机/内网页面时在 `[fetch]` 设置 `allow_private_hosts = true`。`198.18.0.0/15` 是例外：Clash 等本地代理用它作 fake-IP 网段，封禁会导致所有公网域名被误判为内网。重定向由客户端手动跟随（最多 5 跳），每一跳都重新做同样的检查，重定向到非 http(s) 或私有地址会被 `blocked`。残余风险：DNS 重绑定未防护，即首次校验通过后解析结果变化。
- `web_fetch` 对响应体设 10MB 上限，默认只返回前 12000 字符；HTML/text 依次按响应头 `charset`、前 8KB 的 `<meta charset>` 解码，PDF 委托 Firecrawl。需要更多内容时用 `fetch_id` 分页续读，而不是一次请求超大 `max_chars`（超过约 30000 字符可能触及 MCP 客户端的输出上限）。`offset > 0` 而没有 `fetch_id` 会直接返回 `invalid_request`，避免隐式发起新请求。
- `web_search` 默认 5 条来源、每条摘要 800 字符、整响应 30000 字符；这些默认值可在 `[search]` 调整，`snippet_max_chars` 也可单次覆盖。预算裁剪后 `meta.truncated=true`，被省略来源仍可通过 `get_sources` 读取。搜索链为 SearXNG 预留 `[search] reserve_last_resort_ms`（默认 8000ms），付费阶段不会把最后兜底的时间耗尽。`web_map` 同样受 `[map] response_max_chars` 约束，尾部移除的 URL 计入 `omitted_urls`。
- `recency_days` 只接受 1–365；超过一年的值会返回参数错误，不会静默忽略。服务端会按日期做本地二次过滤，但没有日期或日期无法解析的来源会保留。
- 若与 `grok-search-rs` 或 `firecrawl-rotator` 共用 Tavily/Firecrawl key，冷却状态不共享；并发使用可能互相触发 429/额度竞争，建议拆 key 或避免同时跑大流量任务。
- 专用解析器目前覆盖 GitHub issue/PR/discussion、StackExchange 问题页、arXiv abs 页、Wikipedia 条目页；其余页面走通用提取或 Firecrawl。
- PDF 和需要 JavaScript 渲染的页面依赖 Firecrawl；未配置 Firecrawl 时 PDF 会返回 `unsupported_content_type`，需要 JS 的页面通常表现为提取质量低或 `unreachable`。
- 不解析 `robots.txt`，与常见检索型 MCP 服务行为一致；请只抓取你有权访问的页面。
- SearXNG 仅在付费链全部失败或零结果时启用；SearXNG 不可用不会阻止 Tavily/Firecrawl 正常工作，但 `doctor` 会把该检查标为 `unreachable`。
- Windows GNU 目标未安装 rustfmt/clippy 组件；代码未经过 `cargo fmt`。
