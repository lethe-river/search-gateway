---
status: accepted
---

# key 轮询在进程内完成，并带冷却

Tavily 和 Firecrawl 各有多组 key；官方 MCP 都是单 key，现有 firecrawl-rotator 是独立容器。`search-gateway` 在进程内为每个提供方维护 round-robin keyring：遇到 401/402/403/429/432/433 换 key，失败 key 进入冷却窗口，传输层错误原 key 重试一次；全部 key 冷却才做提供方级故障转移。这样 rotator 容器可以退役，且不会对已耗尽的 key 反复发请求。402 被显式纳入，是因为 Firecrawl 用 402 表示额度耗尽，而 grok-search-rs 的判定集不含 402。

## Considered Options

- 保留 rotator 容器：多一个进程和端口，且只能覆盖 Firecrawl。
- 无状态轮询：实现最简单，但额度耗尽的 key 每次都会被再试一遍。
- 多 key 并行发射：故障转移最快，但正常请求也成倍消耗。
- 按响应头额度调度：最精确，但依赖各家头字段，维护成本高。

## Consequences

- 冷却状态只存在进程内，重启清零；这是刻意的，避免状态文件腐化。
- 若某个 402 是账户级而非 key 级，所有 key 会同时冷却，表现为提供方级故障转移。
- 冷却窗口是配置项，需要按实际额度策略调。
