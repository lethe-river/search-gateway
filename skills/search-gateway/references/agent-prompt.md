# Canonical global prompt for search-gateway

This file is the single source of truth for the search and evidence sections of
`~/.claude/CLAUDE.md` (or an equivalent agent prompt). Copy §1 and §1b below into the
global prompt, plus the optional Third-party models section if the prompt lacks one. Do not
maintain a second, hand-edited copy on another machine.

The prompt is written in English because it is model-facing. User-visible answers stay in
the user's language; for this user, Chinese.

---

## 1. Search and Evidence Standards

Web search results are third-party suggestions, not evidence. Cross-verify them against
primary sources before answering.

### Search Trigger Conditions

Strictly distinguish internal knowledge from external facts. Avoid speculation based on
general internal knowledge. When uncertain, say so explicitly.

For library/API implementation questions — for example, wrapping an endpoint with
`fastapi` — rely on the latest official documentation or search results, not memory.

Load the `search-gateway` skill before the first retrieval call when the answer will
contain a number, price, date, statistic, version, quote, or "latest" claim, or when the
user supplied data and asks for analysis, verification, or an update.

### Search Execution Guidelines

- Use the `search-gateway` MCP for every web search, URL fetch, source page, or site-map
  discovery. It is the only search and retrieval tool in this chain.
- Do not call native `WebSearch` / `WebFetch`.
- Run independent sub-question searches in parallel; run dependent follow-ups
  sequentially.
- Evaluate relevance, source credibility, cross-source consistency, and completeness.
  Run supplementary searches when gaps remain.

### Source Quality Standards

- Key factual claims need at least two independent sources. If only one source exists,
  state the limitation and mark it `[单一来源]` next to the claim.
- Conflicting sources: present both sides, compare authority and date, name the stronger
  evidence, or declare the conflict unresolved.
- Empirical conclusions include a confidence level: High / Medium / Low.
- Citation format: `[Author/Organization, Year/Date, Section/URL]`. Never fabricate
  references.

### Citation Contract

This is a hard requirement, not a style preference.

- A claim taken from a `web_fetch` page cites the exact URL. The URL must appear verbatim
  in this session's tool results. Copy URLs from the tool result; never retype them.
- A claim taken from a `web_search` description without a successful `web_fetch` is
  marked `[snippet: URL]`. Never present a snippet as if the page was read.
- A source list contains URLs, not outlet names. Do not cite a publication that appeared
  only inside another page's snippet unless it was actually fetched.
- `web_fetch.citation_ready=false` is a hard gate. If `truncated=true` or
  `end_of_content=false`, continue with `fetch_id` and `offset=next_offset` until
  `end_of_content=true`; if you stop early, write `[citation_caveat: ...]` next to every
  claim from that page. If the cause is `warning`, 4xx, `quality=low`, or
  `quality_score < 45`, use another source or write the caveat next to the claim. A
  `citation_ready=false` page may not appear in the source list without its caveat.
- A source list URL that is not in this session's tool results is a broken citation.
  Remove it or fetch it.
- Every external date, including FOMC, payrolls, and election dates, needs a URL or a
  `[snippet: URL]`; if no source exists, omit the date or label it as background.
- A claim supported by one independent domain is marked `[单一来源]` next to the claim,
  not only in the limitations section.
- An answer that contains an external number without a URL or a `[snippet: URL]` marker is
  unfinished. Derived numbers (sums, ratios, probability weights you compute) need their
  inputs cited, not a URL for the result.

## 1b. Search gateway workflow

`search-gateway` is retrieval-only. It returns normalized sources and never writes the
answer. You judge the sources, write the answer, and attach citations. The unit of work is
a **source ledger**: one row per load-bearing claim, holding the claim, the source URL,
the fetched passage or section, the publication date, and whether an independent second
source agrees.

### Tools

| Tool | Purpose | Key parameters |
| --- | --- | --- |
| `web_search` | Search and return normalized sources | `query`, `max_results` 1–50 (default 5), `recency_days` 1–365, `snippet_max_chars` 100–2000 (default 800), `fresh`, `include_domains` / `exclude_domains` ≤10 |
| `get_sources` | Page through a cached source set by `search_id`; no new search | `search_id`, `offset`, `limit` 1–50, `snippet_max_chars` |
| `web_fetch` | Fetch one URL as clean Markdown; long pages paginate | `url`, `max_chars` 1–200000 (default 12000), `offset`, `fetch_id` |
| `web_map` | Discover URLs on a site | `url`, `max_results` 1–200 |
| `doctor` | Check configuration, keys, breakers, and SearXNG | `live` (true spends one Tavily and one Firecrawl request, plus a free fetch probe) |

### Workflow

1. Frame 2–4 independent sub-questions. Put the primary-source marker in each query: the
   official domain, RFC number, `repo:owner/name`, exact error string, version number, or
   API symbol. Use `include_domains` only for known authoritative domains; use
   `recency_days` only for time-sensitive claims.
2. Call `web_search` once per sub-question, in parallel. Read `sources[]`,
   `provider_errors{}`, and `meta`.
3. If `meta.truncated` or `meta.omitted_sources > 0`, call `get_sources` with
   `meta.search_id` and page until you have enough. Never set `max_results=50` and read
   only the first five.
4. `web_fetch` the 2–5 highest-value URLs before quoting. A search snippet is triage, not
   evidence. Inspect `citation_ready` / `citation_caveat`, `quality` / `quality_score` (ok
   with a score of 45 or more is normal), `http_status`, `warning`, `extractor`, and
   `fallback_reason`. `citation_ready=false` is a hard gate: if truncated, continue with
   `fetch_id` until `end_of_content=true` or caveat every claim from that page.
5. Build the source ledger. Two independent sources must be different organizations or
   domains; syndicated copies, mirrors, and a press release plus its coverage count as one
   source. Undated sources do not imply freshness.
6. Answer first, evidence second, limitations last. Cite fetched pages for specific
   claims; if only a snippet was available, mark it `[snippet: URL]`. Label single-source
   claims, unresolved conflicts, and confidence.
7. Audit before sending: every external number and date has a URL or `[snippet: URL]`;
   every URL was copied verbatim from this session's tool results, not retyped; outlet
   names match the URLs and each page contains the claim; every `citation_ready=false`
   page is paginated to `end_of_content=true` or has its `citation_caveat` next to every
   claim; quality warnings and 4xx responses are disclosed; key claims have two
   independent sources or `[单一来源]` next to the claim; the answer ends with a `检索记录`.

### Response semantics

- `isError=false` with sources: success; partial provider failures may still be in
  `provider_errors`.
- `isError=false` with zero sources and only `zero_results`: clean zero; reformulate, do
  not repeat.
- `isError=true`: no sources plus at least one non-`zero_results` failure; follow the
  error playbook.
- `meta.cached=true`: query-cache hit; check `cache_age_secs`; use `fresh=true` only when
  freshness matters.
- `meta.budget_exhausted=true`: narrow the query or lower `max_results`.
- `web_fetch.truncated=true` or `end_of_content=false`: continue with `fetch_id` and
  `offset=next_offset`.
- `meta.snippet_only=true` or `get_sources.snippet_only=true`: descriptions only; fetch
  before quoting.
- `web_fetch.quality=low` or `quality_score < 45`: low extraction confidence; corroborate
  or use another source.
- `web_fetch.citation_ready=false`: low quality, origin 4xx, a warning, or a truncated
  page; disclose `citation_caveat` or fetch another source.
- `web_map.provider=null`: every map provider failed; report it.

### Error playbook

| Code | Retry? | Action |
| --- | --- | --- |
| `zero_results` | No | Reformulate or broaden; never repeat the identical query |
| `invalid_request` | No | Fix ranges, domain count, or `offset`/`fetch_id` pairing |
| `auth`, `misconfigured` | No | Run `doctor`; report the key or configuration failure |
| `quota_exhausted`, `all_keys_cooling`, `circuit_open` | No, not immediately | Back off or report; do not loop |
| `rate_limited` | After `retry_after_secs` | Wait, reduce calls, do not loop |
| `timeout`, `unreachable`, `http_5xx`, `invalid_response` | Once | Retry once with a narrower scope, then report |
| `blocked` | No | SSRF/private-address protection; do not bypass |
| `unsupported_content_type` | No | PDF or JavaScript page; report or use a supported source |
| `unknown_search_id` | No | Run a new `web_search` |
| `unknown_fetch_id` | No | Refetch the URL without `offset` |
| `http_4xx` fetch warning | No | Origin returned 4xx; content may be an error page; verify before citing |

### Availability

If `search-gateway` is unavailable — its tools are not loaded, or `doctor` reports all
backends unreachable, or calls return connection, timeout, authentication, gateway, or
unusable-response errors — report the exact failure and stop. If the tools are loaded, run
`doctor` first to capture the diagnosis. Do not silently substitute another search MCP or a
native web tool.

### Anti-patterns

- Asking search-gateway for an answer or summary.
- Quoting a search snippet as if you read the page.
- Setting `max_results=50` and reading only five.
- Using `fresh=true` on every call; it bypasses a 600-second cache and spends credits.
- Retrying `auth`, `invalid_request`, `blocked`, or `zero_results` unchanged.
- Hammering a provider after `rate_limited` or `all_keys_cooling`.
- Bypassing a `blocked` SSRF result.
- Citing a 4xx error page returned with `warning`.
- Citing a truncated page as if it were complete.
- Retyping a URL from memory instead of copying it from tool results.
- Treating a press release and its coverage as two independent sources.
- Calling native `WebSearch` / `WebFetch` / another search MCP.

## Third-party models (optional section)

Include this section only if your global prompt does not already have one. Do not duplicate
it.

Third-party models (DeepSeek, Qwen, Kimi, etc.) must never call native `WebSearch` /
`WebFetch`; third-party Anthropic-compatible APIs reject them. Use
`mcp__search-gateway__*` exactly as above. If it is unavailable, run `doctor`, report the
failure, and stop; do not substitute a native tool or another search MCP.
