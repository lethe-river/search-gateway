# 工具参考

五个 MCP 工具的参数语义、响应整形规则、`isError` 契约与错误码。设计依据见
[`adr/0005-agent-response-contract.md`](adr/0005-agent-response-contract.md)。

## web_search

搜索并返回归一化来源。

| 参数 | 取值范围 | 默认 | 说明 |
| --- | --- | --- | --- |
| `query` | 字符串 | 必填 | 搜索词 |
| `max_results` | 1–50 | 5 | 返回来源数上限 |
| `recency_days` | 1–365 | 无 | 时效过滤，见下文 |
| `snippet_max_chars` | 100–2000 | 800 | 单条摘要字符上限，单次覆盖默认值 |
| `fresh` | 布尔 | false | true 时跳过查询缓存 |
| `include_domains` | ≤10 项 | 无 | 只保留这些域名 |
| `exclude_domains` | ≤10 项 | 无 | 排除这些域名 |
| `categories` | `general` / `it` / `science` | `[searxng] categories` | 仅 SearXNG 兜底生效 |

返回 `sources` 数组，以及 `meta.search_id`（供 `get_sources` 使用）、`meta.cached`、
`meta.cache_age_secs`、`meta.chain_used`、`meta.truncated`、`meta.omitted_sources`，
以及 `provider_errors`。

### 搜索缓存分两层

1. **查询缓存**：进程内 LRU，默认 600 秒、最多 32 条。命中时 `meta.cached=true`、
   `meta.cache_age_secs` 给出结果年龄、`meta.chain_used=["cache"]`；需要最新结果时传
   `fresh=true`。命中会复用原来的来源集 `search_id`，不新建缓存条目；来源集被淘汰时自动重建。
2. **来源集缓存**：默认持久化，重启后未过期的 `search_id` 仍有效，用于 `get_sources` 分页。

空结果、或带 `provider_errors` 的结果**不进入**查询缓存。`meta.snippet_only=true`
表示本次每条来源只有搜索摘要——引用数字或具体论断前必须先 `web_fetch`。

### recency_days 的两段式过滤

上限 365。Tavily / Firecrawl / SearXNG 的原生时间过滤只支持 day/week/month/year，
因此服务端还会在归一化阶段按 `published_date` 做一次本地过滤，丢弃日期可解析且早于
`now - recency_days` 的来源。没有日期或日期无法解析的来源会保留，避免丢大量结果。
超过一年的值会被**拒绝**而不是静默忽略。

### include/exclude_domains 与 categories

`categories` 只传给 SearXNG，且 SearXNG 仅作为最后兜底；Tavily / Firecrawl 没有等价参数。
SearXNG 没有原生的 include/exclude 域名参数，服务端会拼成 `site:` / `-site:` 查询词，
并保留本地域名后过滤作为双保险。

## 响应整形与截断预算

响应按整条来源整形：每条摘要默认最多 800 字符，`web_search` 与每个 `get_sources` 页还受
整响应预算约束（默认 30000 字符）。预算从尾部移除来源，返回 `meta.truncated=true` 与
`meta.omitted_sources`；被移除的来源仍在缓存里，可用 `search_id` 继续分页读取。

## get_sources

按 `search_id` 分页读取上一次搜索的完整来源集，**不发起新搜索**。

| 参数 | 取值范围 | 默认 |
| --- | --- | --- |
| `search_id` | 必填 | — |
| `offset` | ≥0 | 0 |
| `limit` | 1–50 | 20 |
| `snippet_max_chars` | 100–2000 | 沿用原搜索的 cap |

响应报告实际生效的 `snippet_max_chars`；任一摘要被 cap 截断、或预算移除本页尾部来源时
`truncated=true`，`next_offset` 指向下一批未返回的来源。同样返回 `snippet_only=true`。
`search_id` 未知或过期 → `isError=true`。

## web_fetch

抓取单个 URL，返回 Markdown。

| 参数 | 取值范围 | 默认 |
| --- | --- | --- |
| `url` | 必填 | — |
| `max_chars` | 1–200000 | 12000 |
| `offset` | ≥0，>0 时必须同时给 `fetch_id` | 0 |
| `fetch_id` | 上一次响应的 fetch_id，须与同一 `url` 配对 | — |

返回字段：`original_length`（完整提取文本字符数）、`quality`（`ok`/`low`/`unknown`）、
`quality_score`（0–100）、`citation_ready`、`citation_caveat`、`fetch_id`、`next_offset`、
`end_of_content`、`http_status`。

### 引用门

`citation_ready=false` 表示低质量、源站 4xx、warning 或页面截断。

- 因**截断**导致：先按 `fetch_id` + `next_offset` 续读到 `end_of_content=true`。
- 其他原因：宿主必须在每个相关论断旁披露 `citation_caveat`，或换来源。
- `citation_ready=false` 的页面**不得**无 caveat 出现在来源列表里。

正文超过 `max_chars` 时 `truncated=true`；把同一 `url`、`fetch_id` 和 `next_offset`
作为 `offset` 再调一次即可读下一段，续读走本地页面缓存，不发起网络请求。
`offset > 0` 缺 `fetch_id` → `invalid_request`（在发网络请求前就返回）。`fetch_id`
与产生它的 `url` 不一致 → `unknown_fetch_id`。

### 页面缓存

默认保存完整正文（最多 100 万字符），有效期由 `[cache] page_ttl_secs` 控制（默认 600 秒），
容量由 `[cache] max_pages` 控制（默认 64 页）。未知或过期 `fetch_id` → `unknown_fetch_id`
且 `isError=true`。

### 源站 4xx 与 Firecrawl 兜底

`http_status` 是最后一次向源站发起请求得到的状态码。源站返回 4xx 但仍有可用内容时，
响应通过 `warning.code=http_4xx` 报告，`error=null`、`isError=false`（便于判断内容是否来自
错误页）。只有 **403/429** 会尝试 Firecrawl 兜底；404/410 等直接返回 `http_4xx`，
不消耗 Firecrawl 额度。

### 提取质量与编码

通用提取的 `quality_score` 低于 **45** 时，若配置了 Firecrawl 会自动改用 Firecrawl 并取
分数更高者。HTML 缺少 charset 头时会读取前 8KB 的 `<meta charset>`；评分对 CJK 字符按
双宽计入，避免短中文正文被误判为低质量。

## web_map

发现站点 URL。

| 参数 | 取值范围 | 默认 |
| --- | --- | --- |
| `url` | 必填 | — |
| `max_results` | 1–200 | 50 |

响应受 `[map] response_max_chars` 约束，尾部被移除的 URL 计入 `omitted_urls`。
未配置 Tavily/Firecrawl 时写入 `provider_errors["map"]`（`misconfigured`）。

## doctor

检查配置、keyring、熔断器、SearXNG 健康与引擎状态。

| 参数 | 说明 |
| --- | --- |
| `live` | true 时向 Tavily 和 Firecrawl 各发一个最小请求（**消耗额度**），并对 `[doctor] fetch_probe_url` 做一次免费抓取探测 |

不带 `live` 时付费探测被跳过，整体状态为 `unknown`、`live_verified=false`；
只有 `misconfigured` 或 `unreachable` 会让 CLI 退出码非零。

## isError 契约

| 工具 | `isError=true` | `isError=false` |
| --- | --- | --- |
| `web_search` | 零来源，且至少一个 `provider_errors` 不是 `zero_results`（鉴权失败、超时、限流、预算耗尽等） | 有来源；或零来源但所有错误都是 `zero_results`（干净的零结果） |
| `web_fetch` | `error` 非空（全部提取失败、URL 非法、被 SSRF 拦截、未知 `fetch_id` 等） | 返回了内容；源站 4xx 但有可用内容时只带 `warning` |
| `web_map` | 没有任何提供方成功（`provider` 为空：未配置或全部失败） | 提供方成功，即使返回零 URL |
| `get_sources` | `search_id` 未知或过期 | `search_id` 有效，即使页为空 |

`web_search` 只要返回了来源就保持 `isError=false`，部分提供方失败放在 `provider_errors` 里。
零结果时会写入 `provider_errors["chain"]`（`zero_results`）；SearXNG 兜底零结果时消息会带上
不可用引擎名。

## 错误码

`unreachable`、`timeout`、`http_5xx`、`http_4xx`、`invalid_request`、`invalid_response`、
`auth`、`quota_exhausted`、`rate_limited`、`all_keys_cooling`、`circuit_open`、
`misconfigured`、`blocked`、`unsupported_content_type`、`zero_results`、`budget_exhausted`、
`unknown_search_id`、`unknown_fetch_id`。

提供方返回 `Retry-After` 时，对应错误会带 `retry_after_secs`；零来源搜索还会在
`meta.retry_after_secs` 给出各错误中的最大值。`http_4xx` 表示源站对抓取返回了 4xx；
`invalid_request` 表示搜索提供方拒绝了本次请求参数——两者都不会触发提供方级熔断。

## 全局限流

每个出站工具调用（搜索、抓取、站点地图）都经过进程级滑动窗口限流，默认每分钟 60 次
（`[rate] max_calls_per_minute = 0` 可关闭）。超限时返回 `provider_errors.local`
（`rate_limited`，带 `retry_after_secs`），不消耗任何提供方额度。
