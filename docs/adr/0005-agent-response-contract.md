---
status: accepted
---

# Agent 响应契约：isError、缓存新鲜度、分页与预算

MCP 客户端主要靠 `isError` 和结构化字段决定下一步动作，而不是解析自然语言。0.2.0 起把这条契约显式化，减少“成功却报错”“内容被截断却无标记”“重复搜索无新鲜度信息”这类歧义。

## isError 矩阵

| 工具 | `isError=true` | `isError=false` |
| --- | --- | --- |
| `web_search` | 零来源，且至少一个 `provider_errors` 不是 `zero_results` | 有来源；或零来源但所有错误都是 `zero_results` |
| `web_fetch` | `error` 非空 | 返回了内容；4xx 但仍有内容时只带 `warning` |
| `web_map` | 没有任何提供方成功（`provider` 为空） | 提供方成功，即使零 URL |
| `get_sources` | `search_id` 未知或过期 | 有效 `search_id` |

干净的零结果是检索的正常结果，不是工具故障。真实故障（鉴权、超时、限流、预算耗尽、全部提供方不可达）仍为 `isError=true`，并保留在 `provider_errors` 中。

## 可见的截断

- `web_search` 的摘要受 `snippet_max_chars` 与整响应 `response_max_chars` 约束，`meta.truncated` / `meta.omitted_sources` 报告裁剪。
- `get_sources` 按原搜索的摘要 cap 返回，可用 `snippet_max_chars` 覆盖；响应返回实际 `snippet_max_chars`，摘要被截断时 `truncated=true`。
- `web_map` 受 `[map] response_max_chars` 约束，尾部移除的 URL 计入 `omitted_urls` 并置 `truncated=true`。
- `web_fetch` 的 `truncated` / `end_of_content` 继续描述分页位置；源站 4xx 但内容可用时通过 `warning` 而非 `error` 报告。

## 缓存新鲜度

查询缓存命中时返回 `meta.cached=true`、`meta.cache_age_secs`，并复用原来的来源集 `search_id`；来源集被淘汰时重建并更新查询缓存指针。`fresh=true` 只跳过查询缓存，不改变缓存键语义。缓存版本升到 v2，持久化写入改为快照 + `spawn_blocking` + 进程内写锁，读-合并-写仍在写锁内完成以避免并发进程互相覆盖。

## 分页与预算

- `web_fetch` 的 `offset > 0` 必须配合 `fetch_id`，否则在发起网络前返回 `invalid_request`。
- 页面缓存独立配置：`[cache] max_pages`、`page_ttl_secs`。
- 搜索为 SearXNG 预留 `[search] reserve_last_resort_ms`，付费阶段使用 `slice_reserving`，不会把最后兜底预算吃光。

## 错误提示

提供方 `Retry-After` 会出现在 `ProviderError.retry_after_secs` / `ToolError.retry_after_secs`；零来源搜索在 `meta.retry_after_secs` 给出最大值。SearXNG 零结果时消息包含不可用引擎名。`doctor` 不带 `live` 时整体状态为 `unknown`、`live_verified=false`，只有 `misconfigured` / `unreachable` 让 CLI 退出码非零。

## 引用门（Citation gate）

`web_fetch` 返回 `citation_ready` 与 `citation_caveat`：

- `citation_ready=true`：提取质量分不低于 45、无 `warning`、源站不是 4xx，且当前片段已到 `end_of_content`；宿主可以直接引用。
- `citation_ready=false`：低质量、源站 4xx、存在 `warning` 或页面截断；`citation_caveat` 给出具体原因（多项原因用分号拼接）。截断时先按 `fetch_id` + `next_offset` 续读到 `end_of_content=true`；否则宿主必须在每个相关论断旁披露 caveat，或换一个来源。未披露 caveat 的 `citation_ready=false` 页面不得进入来源列表。

`web_search` 的 `meta.snippet_only` 与 `get_sources.snippet_only` 恒为 `true`：这些来源只有摘要，没有正文。引用数字或具体论断前必须 `web_fetch`。这两个字段是显式提示，避免把 `content: null` 当成已读正文。

新增字段是 additive change，但旧客户端看不到；本 ADR 是 0.2.0 契约的一部分。

## Considered Options

- 保持 `isError = sources.is_empty()`：实现简单，但干净的零结果被当成故障，客户端会重试或换工具。
- 把 4xx 内容同时放在 `error` 和内容里：调用方能拿到内容，但 `isError=true` 会让客户端丢弃结果。
- 让 `get_sources` 永远用配置的摘要 cap：调用方无法恢复搜索时请求的更长摘要，且没有截断标记。

## Consequences

- 0.2.0 是破坏性契约变更：新增必填字段（`meta.cached`、`meta.cache_age_secs`、`meta.snippet_only`、`get_sources.snippet_max_chars`、`get_sources.truncated`、`get_sources.snippet_only`、`web_fetch.warning`、`web_fetch.citation_ready`、`web_fetch.citation_caveat`、`web_map.omitted_urls`、`doctor.live_verified`），缓存格式 v2。
- 客户端应以 `isError` 判断调用是否失败，以 `provider_errors` / `warning` 判断部分降级，以 `truncated` / `omitted_*` 判断是否需要继续分页。
- 4xx 抓取只在 403/429 时尝试 Firecrawl，其他 4xx 不再消耗付费额度。
