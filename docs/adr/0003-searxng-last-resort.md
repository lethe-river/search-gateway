---
status: accepted
---

# SearXNG 作为最后一级兜底

`search-gateway` 的搜索链是 Tavily（轮询/冷却）→ Firecrawl（轮询/故障转移）→ SearXNG。SearXNG 只在 Tavily 与 Firecrawl 都失败或归一化后零来源时启用，不参与补位，不参与抓取。它返回的 `content` 只作为 `Source.description`，正文仍由宿主走抓取链。

客户端用 `categories` 选引擎，不硬编码引擎名；服务端启用 `google cse` / `bing` / `360search`，关闭 `brave` / `duckduckgo` / `startpage`，并用 `timeout_limit` + `max_request_timeout` 控制延迟。连续 3 次瞬态失败后熔断 300s，冷却后半开探测；0 结果不计熔断失败。

## Considered Options

- 不设兜底：链路只到 Firecrawl；两者同时失败或额度耗尽时直接报错。最简单，但失去独立最后一级。
- 把 SearXNG 嵌进 Rust 二进制：需要打包 Python/Flask 与 274 个引擎并长期维护，成本远高于收益。
- 用 Rust 重写 Bing/360/Google CSE 抓取：省掉容器，但重复 SearXNG 的引擎维护与反爬工作，不适合只做兜底。
- 复用现有 SearXNG（采纳）：零新增容器，保留最后一级；代价是依赖一个本地 Python 服务。

## Consequences

- 12310 是外部依赖；Docker/WSL 不在时熔断打开，doctor 报 `unreachable`。
- `google cse` 依赖第三方公共 CSE，是单点；bing/360 是冗余，不是保证。
- SearXNG 只给 snippet；需要正文时抓取链仍要单独执行。
- 引擎可能随时改版或限流；兜底是 best-effort，不承诺 SLA。
