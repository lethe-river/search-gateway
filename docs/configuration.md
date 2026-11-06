# 配置

## 配置路径优先级

1. `--config PATH`
2. 环境变量 `SEARCH_GATEWAY_CONFIG`
3. OS 默认路径：Windows `%APPDATA%\search-gateway\config.toml`；
   Linux/macOS（有 `HOME` 时）`~/.config/search-gateway/config.toml`
4. 仅当 `APPDATA` 和 `HOME` 都未设置时，才使用当前目录 `config.toml`
5. 所选文件不存在时：服务模式直接报错；`--doctor` / `--selftest` 使用内置默认值并给出警告

**当前目录的 `config.toml` 不是 OS 默认路径之后的兜底。** 要使用仓库内或项目内的配置，
必须显式传 `--config`，或设置 `SEARCH_GATEWAY_CONFIG`。

配置文件不会自动创建。服务模式（不带 `--doctor` / `--selftest`）启动时若配置无效会直接
报错，并把示例配置打印到错误信息里。

## 最小可用

```toml
[searxng]
enabled = true
base_url = "http://127.0.0.1:12310"
```

只走本地 SearXNG，零 API 费用。加上 Tavily / Firecrawl 才会启用付费链与故障转移。

## 完整示例

字段与默认值如下，完整注释版见 [`../config.example.toml`](../config.example.toml)。

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
enabled = false
base_url = "http://127.0.0.1:12310"
connect_timeout_ms = 2000
request_timeout_ms = 8000
server_timeout_s = 6
categories = ["general"]
language = "auto"
max_results = 10
required_engines = []
disabled_engines = []

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
tavily_timeout_ms = 10000
firecrawl_timeout_ms = 12000

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

## Key 解析

`keys` 的每一项可以是字面值，也可以是 `env:NAME`。一个环境变量的值还可以是用逗号、
分号或空白分隔的多个 key：

```toml
[tavily]
keys = ["env:TAVILY_API_KEY"]
# TAVILY_API_KEY="tvly-aaa,tvly-bbb,tvly-ccc" 会解析成 3 个 key
```

多 key 按轮询使用：

| 响应 | 冷却时长 | 配置字段 |
| --- | --- | --- |
| `401` / `403` | 600 秒 | `[keyring] auth_cooldown_secs` |
| `402` / `432` / `433` | 3600 秒 | `[keyring] quota_cooldown_secs` |
| `429` | 60 秒 | `[keyring] rate_limit_cooldown_secs` |

响应带 `Retry-After` 时优先使用它。同一提供方的 keyring 在搜索、抓取、站点地图和 `doctor`
之间共享；熔断器按操作区分（`tavily.search`、`tavily.map`、`firecrawl.search`、
`firecrawl.scrape`、`firecrawl.map`、`searxng.search`），一个操作失败不会阻塞另一个。
所有 key 都在冷却时该提供方直接故障转移，**不**打开熔断器。

`keys` 为空时会回退到旧环境变量 `TAVILY_API_KEY` / `FIRECRAWL_API_KEY`。
key 不会出现在日志或 `doctor` 输出中，`doctor` 只报告数量和健康状态。

## 与其他客户端共用 key

若与其他工具（例如另一个搜索 MCP 或自建的 key 轮换代理）共用 Tavily / Firecrawl 的 key，
**冷却状态不共享**：并发使用可能互相触发 429 或额度竞争。建议拆开 key，或避免同时跑
大流量任务。
