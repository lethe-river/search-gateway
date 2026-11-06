---
name: search-gateway
description: Retrieve web sources with search-gateway and turn them into a cited, auditable answer. Use for any task that needs external facts, prices, dates, statistics, news, documentation, API behavior, exact quotes, fact-checking, or source verification — including market and trend analysis — or when the user says 搜索、查资料、找来源、核实、抓取网页、行情、价格、趋势、新闻分析、数据分析. Load before the first search whenever the answer will contain numbers or time-sensitive claims. It is retrieval-only; the host model writes the answer.
---

# Search Gateway

search-gateway is a **retrieval-only** MCP server: it returns normalized sources and never writes an answer. You are the host model — you judge the sources, write the answer, and attach citations.

The unit of work is a **source ledger**: one row per load-bearing claim, holding the claim, the source URL, the fetched passage or section, the publication date, and whether an independent second source agrees.

## Before you search

Load this skill before the first retrieval call when any of these is true:

- The answer will contain a number, price, date, statistic, version, quote, or "latest" claim.
- The user supplied data and asks you to analyze, verify, or update it; the pasted data is a lead, not evidence.
- The task is market, trend, news, earnings, policy, or event analysis and the conclusion depends on current facts.
- The user says 搜索、查资料、找来源、核实、最新、行情、走势、价格.

Do not treat an analysis-shaped request as an exemption from retrieval. If external facts decide the answer, retrieve them.

## Citation contract

This contract is mandatory. It is the part most often skipped, and the part the user audits.

1. **Fetched claim** — a claim taken from a `web_fetch` page cites the exact URL: `[Organization, Date, Section](URL)`. The URL must appear verbatim in this session's tool results.
2. **Snippet-only claim** — a claim taken from a `web_search` description without a successful `web_fetch` is marked `[snippet: URL]`. Never present it as if the page was read.
3. **Single-source claim** — marked `[单一来源]` (or `[single source]` in English) next to the claim, not only in the limitations section, and repeated in the limitations.
4. **Source list** — every entry is a URL, not an outlet name. Do not list a publication that appeared only inside another page's snippet unless you actually fetched it.
5. **Citation gate** — if `web_fetch.citation_ready=false`, the page is not ready to cite.
   - `truncated=true` or `end_of_content=false`: continue with `fetch_id` and `offset=next_offset` until `end_of_content=true`. If you stop early, write `[citation_caveat: <citation_caveat>]` next to every claim taken from that page.
   - `warning`, `http_status` 4xx, `quality = low`, or `quality_score < 45`: use another source, or write the caveat next to the claim.
   - A `citation_ready=false` page may not appear in the source list without its caveat.
6. **URL exactness** — copy the URL exactly from tool results; never retype it from memory. Before sending, search the draft for each URL; if it does not appear in this session's tool results, delete it or fetch it.
7. **Dates** — every external date, including event dates such as FOMC, payrolls, and elections, needs a URL or `[snippet: URL]`. If no source exists, omit the date or label it as background.
8. **No fabrication** — never invent a URL, date, section, or author. If the URL is unknown, write `[snippet: search result, URL unavailable]` and say the evidence is incomplete.

An answer that contains an external number without one of these markers is unfinished. Derived numbers (sums, ratios, probability weights you compute) need their inputs cited, not a URL for the result.

## Scope

`search-gateway` is the only search and retrieval tool in this chain.

| Need | Tool |
| --- | --- |
| All web retrieval: sources, citations, exact quotes, source inventory, pagination, controlled fetch, site map | `mcp__search-gateway__*` |
| A synthesized overview | Retrieve with search-gateway, then synthesize in your own answer |
| search-gateway unavailable per explicit failure | Run `doctor`, report the failure, and stop |

- Never call native `WebSearch` / `WebFetch`.
- Never ask search-gateway to summarize or rank "the best" source; it only retrieves.
- Do not use another search MCP as a fallback. If `search-gateway` is unavailable, diagnose it with `doctor` instead of routing around it.
- If the global `CLAUDE.md` still routes searches differently, replace its §1/§1b with `references/agent-prompt.md`. Do not maintain a second policy copy.

## The loop

### 1. Frame the question

Split the request into 2–4 independent sub-questions. Independent queries run in parallel; a follow-up that depends on an earlier answer runs after it.

Put the primary-source marker in the query: the official domain, `RFC 9110`, `repo:owner/name`, the exact error string, the version number, or the API symbol.

Use `include_domains` only when you already know the authoritative domains. Use `recency_days` only when the claim is time-sensitive — news, prices, releases, "latest". Evergreen topics omit it.

Completion: every sub-question has a query and a success criterion, such as "the official docs page for X" or "two independent reports of Y".

### 2. Search

Call `web_search` once per sub-question, in parallel.

Read every response before the next step:

- `sources[]`: `url`, `provider`, `title`, `description`, `published_date`.
- `provider_errors{}`: partial degradation; one provider failing does not fail the search.
- `meta.chain_used`: `tavily` → `firecrawl` → `searxng`; `["cache"]` means a reused query.
- `meta.cached` / `meta.cache_age_secs`: result age.
- `meta.truncated` / `meta.omitted_sources`: the response budget cut tail sources.
- `meta.search_id`: keep it for `get_sources`.
- `meta.budget_exhausted`: the 30-second search budget ran out; narrow the query.

`isError=false` with zero sources is a clean zero result, not a failure. `isError=true` means no sources and at least one non-`zero_results` provider error.

Completion: every sub-question has at least one candidate, or a clean zero is confirmed and you can say why.

### 3. Expand the candidate set

- If `meta.truncated` or `meta.omitted_sources > 0`, call `get_sources` with `{search_id, offset: 0, limit: 50}`; follow `next_offset` until you have enough or the page is empty.
- If coverage is thin, issue new queries with different terms; do not repeat an identical query without changing parameters. `fresh=true` is not a fix for bad terms.
- Prefer primary sources: official docs, source code, specs, first-party announcements, the actual issue or pull request. Secondary write-ups are leads, not evidence.
- Completion: each key claim has at least two independent candidates, or you know why only one exists.

### 4. Fetch before quoting

- `web_fetch` the 2–5 highest-value URLs. Do not quote a claim from a search snippet alone; snippets are for triage.
- Default `max_chars=12000`; use 3000–6000 for a quick check. For a long page, keep `fetch_id` and pass `offset=next_offset`; stop at `end_of_content=true`.
- `offset > 0` without `fetch_id` is rejected before any network request — always continue from a previous response.
- Inspect `citation_ready` / `citation_caveat`: **`citation_ready=false` is a hard gate, not a hint.**
  - If `truncated=true` or `end_of_content=false`, continue with `fetch_id` and `offset=next_offset` until `end_of_content=true`. If you stop early, write `[citation_caveat: ...]` next to every claim from that page.
  - If the cause is `warning`, 4xx, `quality=low`, or `quality_score < 45`, use another source or write the caveat next to the claim.
- Inspect `quality` / `quality_score`: `ok` with a score of 45 or more is normal; `low` or a score below 45 means read cautiously or corroborate.
- Inspect `http_status` and `warning`: a 4xx with content is a warning, not a clean success; the content may be an error page.
- Inspect `extractor` and `fallback_reason`: specialist, generic, or Firecrawl.
- Specialist extractors cover GitHub issues/PRs/discussions, StackExchange, arXiv abs pages, and Wikipedia. Other pages use generic extraction; PDF and JavaScript-heavy pages need Firecrawl.
- `blocked` means SSRF/private-address protection. Do not try to bypass it; ask the user only if the local page is the actual target.
- Completion: every load-bearing claim has a fetched passage with URL, section or heading, and date. Every `citation_ready=false` page is either paginated to `end_of_content=true` or its caveat is written next to the claim.

### 5. Build the source ledger

For each claim:

- Two independent sources mean different organizations or domains. Syndicated copies, mirrors, and a press release plus its coverage are one source, not two.
- Conflicting sources: present both, compare authority and date, name the stronger one, or declare the conflict unresolved.
- Undated sources: do not imply freshness; say "date unavailable".
- Single-source claims: label them `[单一来源]` immediately after the claim and repeat the limitation in the answer. A sentence in the limitations section alone does not satisfy this.
- Empirical conclusions: attach confidence High / Medium / Low and the reason.
- Completion: every claim in the answer is either in the ledger or marked unsupported.

### 6. Write the answer

- Answer first, evidence second, limitations last.
- Output in the user's language; for this user, Chinese. Cite as `[Author/Organization, Year/Date, Section/URL]`.
- For a specific claim, cite a URL you fetched. If only a snippet was available, mark it `[snippet: URL]`.
- Copy URLs from the source ledger; do not write the source list from memory.
- Do not fabricate references. If the evidence is thin, say so.
- End with a `检索记录`: queries, providers, fetched URLs, and snippet-only claims. This makes the retrieval auditable.

### 7. Audit before sending

Run this checklist against the draft. If any answer is "no", fix the draft, not the checklist.

| Check | Pass condition |
| --- | --- |
| Number coverage | Every external number, price, date, and statistic has a URL or a `[snippet: URL]` marker |
| URL provenance | Every URL was copied verbatim from this session's `web_search` / `web_fetch` / `get_sources` results; none was retyped |
| Source identity | The outlet named next to a URL is the outlet that publishes that URL, and the page actually contains the claim |
| Fetch quality | `quality_score < 45`, `quality=low`, `warning`, and `http_4xx` are disclosed next to the claim |
| Citation gate | Every `citation_ready=false` page is paginated to `end_of_content=true` or has its `citation_caveat` next to every claim |
| Independence | Every key claim has two independent sources or a `[单一来源]` marker next to it |
| Conflicts | Conflicting figures are shown with authority/date, or declared unresolved |
| Retrieval record | Queries, providers, fetched URLs, and snippet-only claims are listed |

A draft that fails number coverage, URL provenance, source identity, the citation gate, or independence is not ready. See `references/answer-quality.md` for the scoring rubric and common failure modes.

**Before sending, run this mechanical pass:**

1. Extract every external number and date from the draft. For each, write the URL and whether it was fetched or snippet-only. For a table, check every row separately; a source paragraph below the table does not cover the row.
2. Search the session's tool results for every URL in the draft. A URL that is not there is a broken citation; remove it or fetch it.
3. For every `web_fetch` with `citation_ready=false`, locate the claims it supports. Either paginate to `end_of_content=true`, or put `citation_caveat` next to each claim.
4. Count independent domains for every key claim. One domain means `[单一来源]` next to the claim. If you cannot name a second independent domain, label it; do not leave it unmarked.
5. For every derived number (converted tonnage, percentage, weighted score), write the formula or the inputs inline; "derived" is not a citation.
6. Use the literal `[snippet: URL]` marker for snippet-only claims. A prose note such as `[Source, Date, snippet]` is not the marker.
7. Append `检索记录`. If a claim has no marker and no URL, the draft is unfinished.

## Tool reference

### web_search

Search the web and return normalized sources. `content` is always null; use `web_fetch` for text.

| Parameter | Type / range | Default | Use |
| --- | --- | --- | --- |
| `query` | string | required | Put the primary-source marker in it |
| `max_results` | 1–50 | 5 | 5 triage, 10–20 coverage, 50 only if you will page |
| `recency_days` | 1–365 | none | Local date filter; undated sources remain |
| `snippet_max_chars` | 100–2000 | server setting (800) | 300–500 triage; 1200–2000 when the snippet is all you read |
| `fresh` | bool | false | Bypass the 600-second query cache; not a fix for bad terms |
| `include_domains` | ≤10 | none | Strict on Tavily; SearXNG receives `site:` terms |
| `exclude_domains` | ≤10 | none | Strict on Tavily; SearXNG receives `-site:` terms |
| `categories` | `general`, `it`, `science` | server setting | SearXNG last resort only |

Returns `query`, `sources[]` (`url`, `provider`, `title`, `description`, `published_date`, `content`), `provider_errors{}`, and `meta{elapsed_ms, chain_used, budget_exhausted, result_count, omitted_sources, truncated, snippet_only, cached, cache_age_secs, retry_after_secs?, search_id, request_id}`. `meta.snippet_only=true` means every source carries only a search snippet; fetch before quoting.

### get_sources

Page through the full source set cached by a previous `web_search`. It does not issue a new search.

| Parameter | Type / range | Default | Use |
| --- | --- | --- | --- |
| `search_id` | string | required | From `meta.search_id` |
| `offset` | integer | 0 | Use `next_offset` from the previous page |
| `limit` | 1–50 | 20 | 50 when you need the rest |
| `snippet_max_chars` | 100–2000 | original search cap | Raise it to recover longer descriptions |

Returns `search_id`, `total_sources`, `snippet_max_chars`, `sources[]`, `next_offset`, `truncated`, `snippet_only`, and `error`. `snippet_only=true` means the page still carries only descriptions; an unknown or expired `search_id` is a tool error.

### web_fetch

Fetch one URL and return clean Markdown. The chain is specialist extractor → generic readability → Firecrawl when local extraction scores low.

| Parameter | Type / range | Default | Use |
| --- | --- | --- | --- |
| `url` | absolute http(s) | required | Do not pass a private/loopback address |
| `max_chars` | 1–200000 | 12000 | 3000–6000 for a quick check; 12000 for a normal read |
| `offset` | integer | 0 | Continuation; requires `fetch_id` |
| `fetch_id` | string | none | From the previous response; must pair with the same `url` |

Returns `url`, `title`, `content`, `original_length`, `truncated`, `end_of_content`, `http_status`, `extractor`, `quality`, `quality_score`, `citation_ready`, `citation_caveat`, `fetch_id`, `next_offset`, `fallback_reason`, `warning`, and `error`. `citation_ready=false` means low quality, origin 4xx, a warning, or a truncated page; disclose `citation_caveat` or fetch another source. Only 403/429 trigger a Firecrawl fallback; 404/410 and other 4xx return `http_4xx` without spending Firecrawl credits.

### web_map

Discover URLs on a website through Tavily Map, then Firecrawl Map. There is no SearXNG map path and no pagination.

| Parameter | Type / range | Default | Use |
| --- | --- | --- | --- |
| `url` | base URL | required | Narrow the base to the section you need |
| `max_results` | 1–200 | 50 | 100–200 for a full inventory |

Returns `base_url`, `urls[]`, `provider`, `truncated`, `omitted_urls`, and `provider_errors`. When `provider` is null, every map provider failed.

### doctor

Check configuration, key rings, circuit breakers, and SearXNG health.

| Parameter | Type | Default | Use |
| --- | --- | --- | --- |
| `live` | bool | false | `true` spends one Tavily and one Firecrawl request, plus a free fetch probe |

Returns `status` (`ok`, `degraded`, `unknown`, `unreachable`, `misconfigured`), `live_verified`, `checks[]`, `breaker{}`, and `keyring{}`. Without `live=true`, skipped paid probes make the status `unknown`; that is expected, not a failure.

## Response semantics

| Signal | Meaning | Action |
| --- | --- | --- |
| `isError=false`, sources present | Success; partial provider failures may still exist | Check `provider_errors` |
| `isError=false`, zero sources, all errors `zero_results` | Clean zero result | Reformulate; report no evidence if every query is zero |
| `isError=true` | No sources plus at least one non-`zero_results` failure | Follow the error playbook |
| `provider_errors[key]` | That provider failed; the chain may have continued | Retry only if `retryable` and worthwhile |
| `meta.cached=true` | Reused query cache | Check `cache_age_secs`; use `fresh=true` if too old |
| `meta.truncated` / `omitted_sources` | Response budget cut tail sources | `get_sources` with `search_id` |
| `meta.budget_exhausted` | Search budget exhausted | Narrow the query or lower `max_results` |
| `web_fetch.warning` | Non-fatal issue, e.g. origin 4xx with usable content | Inspect `http_status`; do not cite blindly |
| `web_fetch.error` | Total extraction failure | Try another URL or a fallback tool |
| `meta.snippet_only=true` / `get_sources.snippet_only=true` | Descriptions only; no fetched prose | `web_fetch` before quoting |
| `web_fetch.quality=low` / score <45 | Low extraction confidence | Corroborate or use another source |
| `web_fetch.citation_ready=false` | Low quality, origin 4xx, warning, or truncated page | Disclose `citation_caveat` or fetch another source |
| `web_fetch.truncated=true` / `end_of_content=false` | More content available | Continue with `fetch_id` and `next_offset` |
| `web_map.provider=null` | Every map provider failed | Use the error playbook; try another approach |
| `web_map.truncated` / `omitted_urls` | Response budget cut URLs | Narrow the base URL or raise `max_results` |
| `get_sources.error=unknown_search_id` | Cache expired or unknown | Run a new `web_search` |
| `doctor.status=unknown`, `live_verified=false` | Paid probes skipped, not a failure | Run `live=true` only when needed |

## Error playbook

| Code | Retry? | Action |
| --- | --- | --- |
| `zero_results` | No | Clean zero; reformulate or broaden; do not repeat the identical query |
| `invalid_request` | No | Fix the parameters (ranges, domain count, `offset`/`fetch_id` pairing) |
| `auth` | No | Run `doctor`; the key is invalid; report it |
| `quota_exhausted` | No, not immediately | The key pool is exhausted; wait or fall back; run `doctor` |
| `rate_limited` | After `retry_after_secs` | Back off; reduce calls; do not loop |
| `all_keys_cooling` | After cooldown | The provider is unusable until keys recover; fall back |
| `circuit_open` | After cooldown | The provider breaker is open; fall back; run `doctor` |
| `timeout`, `unreachable`, `http_5xx` | Once | Retry once with a narrower scope, then fall back |
| `invalid_response` | Once | The provider returned malformed data; retry once, then fall back |
| `budget_exhausted` | With a smaller request | Lower `max_results` or `snippet_max_chars`; one query at a time |
| `misconfigured` | No | Run `doctor`; a provider or key is missing, or the URL is wrong |
| `blocked` | No | SSRF/private-address protection; do not bypass |
| `unsupported_content_type` | No | PDF or JavaScript page needs Firecrawl; use a fallback tool or report it |
| `unknown_search_id` | No | Run a new `web_search`; the source set expired |
| `unknown_fetch_id` | No | Refetch the URL without `offset`; the page cache expired |
| `http_4xx` (fetch warning) | No | The origin returned 4xx; the content may be an error page; verify |

## Recipes

| Task | Call |
| --- | --- |
| Quick fact check | 2 queries × `max_results=8`, `snippet_max_chars=400`; fetch the top 1–2 |
| Official docs / API | `include_domains` official docs, `max_results=10`; fetch the exact page |
| Recent news | `recency_days=3–7`, `fresh=true`; fetch 2–3 independent outlets |
| Academic paper | `include_domains=["arxiv.org"]` or venue domains; fetch the abstract page; PDF needs Firecrawl |
| GitHub issue / PR | Query the repo and exact error; `web_fetch` the issue/PR URL (specialist extractor) |
| Site inventory | `web_map` with `max_results=100`, then fetch selected URLs |
| Long page | `web_fetch` with `max_chars=12000`; continue with `fetch_id` and `next_offset` |
| Broad source set | `max_results=20`; `get_sources` with `limit=50` to page the rest |

## Anti-patterns

- Asking search-gateway for an answer or summary.
- Quoting a search snippet as if you read the page.
- Setting `max_results=50` and reading only the first five.
- Using `fresh=true` on every call; it bypasses a 600-second cache and spends credits.
- Retrying `auth`, `invalid_request`, `blocked`, or `zero_results` unchanged.
- Hammering a provider after `rate_limited` or `all_keys_cooling`.
- Calling native `WebSearch` / `WebFetch`.
- Bypassing a `blocked` SSRF result.
- Citing a 4xx error page returned with `warning`.
- Citing a truncated page as if it were complete.
- Retyping a URL from memory instead of copying it from tool results.
- Treating a press release and its coverage as two independent sources.

## Answer template

```markdown
## 结论
...

## 证据
| 论断 | 来源 | 日期 | 证据位置 | 独立性 |
| --- | --- | --- | --- | --- |
| ... | [组织, 日期, 章节/URL] | ... | ... | 独立 / 单一来源 |

## 局限与置信度
- 置信度：高/中/低，理由...
- 未解决的分歧：...
- citation_ready=false 页面：[citation_caveat: ...]（若未继续分页）
```

## Deployment notes

This skill is portable. Do not put machine-specific paths, ports, or keys in it.

- The server config path is resolved in this order: `--config PATH`, `SEARCH_GATEWAY_CONFIG`, then the OS default (`%APPDATA%\search-gateway\config.toml` on Windows, `~/.config/search-gateway/config.toml` on Linux/macOS when `HOME` is set). `config.toml` in the working directory is used only when neither `APPDATA` nor `HOME` is set; it is **not** a fallback after the OS path. Pass `--config` if you want a repo-local file.
- Run `mcp__search-gateway__doctor` to discover the actual provider, keyring, breaker, and SearXNG state instead of assuming a local port.
- `get_sources` does not spend provider credits. `doctor` without `live` does not spend Tavily/Firecrawl credits but may probe SearXNG.
- Machine-local deployment details (ports, proxy ranges, key sources) belong in a local `references/local-environment.md` that is not shipped with this repo. The `198.18.0.0/15` fake-IP exception is documented in the README's fetch-boundary notes.
- Install and verification steps are in `references/portable-setup.md`; the canonical global prompt is `references/agent-prompt.md`; the answer rubric is `references/answer-quality.md`.
