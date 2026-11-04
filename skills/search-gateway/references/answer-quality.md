# Answer quality rubric

Use this rubric after retrieval and before sending. It turns the citation contract into a
small number of checks that can be applied to a draft.

## Grades

| Grade | Definition | Typical outcome |
| --- | --- | --- |
| **A — auditable** | Every external number, price, date, and statistic has a URL or a `[snippet: URL]` marker. Every URL appeared in this session's tool results. Quality warnings are disclosed. Key claims have two independent sources or `[单一来源]`. | The reader can click through every load-bearing claim. |
| **B — usable** | Key claims are fetched and cited. A few minor external numbers are snippet-only but labeled. One conflict or quality caveat may be missing. | The answer is usable, but the reader cannot audit every external number. |
| **C — weak** | Key claims rely on search snippets. The source list is a list of outlet names without URLs, or contains sources that were never fetched. | The answer may be correct, but it is not evidence-backed. |
| **D — fail** | A URL is fabricated or misattributed, a snippet is presented as a fetched page, or a cited source does not exist in the session. | Do not send. Re-retrieve. |

## Checklist

| # | Check | Pass condition | Fail action |
| --- | --- | --- | --- |
| 1 | Number coverage | Every external numeric/date/statistic claim has a URL or `[snippet: URL]` | Fetch the source, or mark the claim |
| 2 | URL provenance | Every URL in the draft appears in this session's `web_search`, `get_sources`, or `web_fetch` results | Remove the URL or retrieve it |
| 3 | Source identity | The outlet named next to a URL actually publishes that URL | Correct the attribution |
| 4 | Fetch quality | `citation_ready=false`, `citation_caveat`, `quality_score < 45`, `quality=low`, `warning`, `http_4xx` are disclosed | Add a caveat next to the claim |
| 5 | Independence | Key claim has two independent sources, or `[单一来源]` | Add a second source or label it |
| 6 | Conflicts | Conflicting figures show authority/date, or are declared unresolved | Present both sides |
| 7 | Retrieval record | Queries, providers, fetched URLs, snippet-only claims are listed | Append `检索记录` |
| 8 | URL exactness | Every URL is copied verbatim from a tool result; none is retyped | Remove the URL or fetch it |

Derived numbers (sums, ratios, weighted scores, probability weights) do not need their own
URL, but their inputs must be cited. State the formula or weights so the derivation is
reproducible.

## Common failure modes

These are drawn from the 2026-11-03 verification session (`c4d167fe`).

1. **Outlet list instead of URLs.** The answer ended with `WGC; Kitco; Al Jazeera; ETF.com;
   ...` and no links. A reader cannot tell which page supports which number.
2. **Snippet promoted to evidence.** The 10/30 Au99.99 close and the weekly volume change
   came from search descriptions, were not fetched, and were not marked `[snippet]`.
3. **Misattributed outlet.** A China.com article was listed as `Sina Finance`; a 75% Fed
   probability from Mitrade was listed under `Reuters/TheStreet`.
4. **Silent quality warning.** An `etf.com` fetch returned `http_status=403` and
   `warning.code=http_4xx`; the numbers were used without disclosure.
5. **Primary-source fetch failed but the claim stayed.** The WGC China page fetch returned
   only disclaimers. The answer still implied the WGC page was read; the actual support
   came from Al Jazeera.
6. **Unmarked single source.** A Facebook post was the only source for a weekly ETF flow
   figure. The final answer dropped the figure, which was correct; if it had kept it, it
   needed `[单一来源]`.
7. **Analysis-shaped request bypassed the skill.** The model never loaded
   `search-gateway`; it followed the global prompt only. The retrieval mechanics were
   fine, but the citation audit was skipped.
8. **Citation gate ignored.** A page returned `citation_ready=false` with an origin 4xx
   caveat; the number was cited without the caveat. Treat `citation_ready` as the tool's
   own audit, not an optional hint.
9. **Broken URL from retyping.** A URL was typed from memory; one character differed from
   the tool result, so the link 404s and the cited page was never in the session.
10. **Truncated page treated as complete.** `citation_ready=false` with a truncation
    `citation_caveat` was returned; the answer cited the page without caveat and without
    continuing to `end_of_content=true`.
11. **Source-claim mismatch.** A number was attributed to a fetched page that did not
    contain it, while another fetched page did. Check that the page actually contains the
    number, not only that the outlet name is plausible.
12. **Single-source unlabeled.** A claim supported by one domain appeared without
    `[单一来源]`; a note in the limitations section is not a substitute for a marker next
    to the claim.
13. **Date without source.** FOMC, payrolls, election, or release dates appeared without a
    URL or `[snippet: URL]`. A date is an external fact like any other.
14. **Derived number without formula.** A converted tonnage or percentage was presented as
    if it were a source figure; the inputs were cited but the calculation was not shown.
    Write `89.37亿 / 837.21 ≈ 10.68吨`, or use the source's rounded `1.07万千克`.
15. **Table row without its own source.** A table of signal readings had no source column;
    the sources appeared only in a paragraph below. Put the URL or `[snippet: URL]` in the
    row, or mark the row as derived.

## Self-audit procedure

1. Extract every external number, date, and statistic from the draft. Keep derived numbers
   separate; they need their inputs cited and the formula or inputs shown, not their own
   URL. Check each table row separately; a source paragraph below the table does not cover
   the row.
2. For each one, write the URL and whether it was fetched or snippet-only.
3. Any row without a URL: fetch it, replace it with a fetched source, or mark it
   `[snippet: URL]`.
4. Every URL in the draft: search the session's tool results for the exact string. If it is
   not there, remove it. Do not retype URLs; copy them.
5. Every `web_fetch` with `citation_ready=false`: if it is truncated, continue to
   `end_of_content=true`; otherwise put `citation_caveat` next to every claim from that
   page. Also handle `quality_score < 45`, `quality=low`, `warning`, and 4xx.
6. Every key claim: count independent domains. One domain means `[单一来源]` next to the
   claim, not only in the limitations.
7. Append the `检索记录` block. If the record is longer than the answer, the retrieval was
   too broad; narrow it next time.

## Citation examples

Good:

```markdown
9月全球黄金ETF净流入 100 亿美元，Q3 创纪录 310 亿美元
[World Gold Council, 2026-10-07, Highlights](https://www.gold.org/goldhub/research/gold-etfs-holdings-and-flows/2026/10)
```

Snippet-only:

```markdown
10月30日 Au99.99 收 854.76 元/克 [snippet: https://3g.china.com/act/redian/13004758/20261101/49831346.html]
```

Single source:

```markdown
10月最后一周全球黄金ETF流出约 38 吨 [单一来源] [snippet: https://www.facebook.com/...]
```

Bad:

```markdown
主要来源：WGC、Kitco、Al Jazeera、ETF.com、Reuters。
```
