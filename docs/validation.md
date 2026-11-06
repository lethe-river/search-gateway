# 一次真实检索任务的复盘

- 日期：2026-11-03
- 对象：`search-gateway` MCP + 配套 skill
- 样本：黄金趋势与国内价格分析。用户粘贴 4 个指标（央行购金、ETF、美元/10Y、上金所
  Au99.99），要求分析后续涨跌并列出概率。
- 转录：本地会话记录（未提交）

这份文档保留原样结论，包括对自己不利的部分。评分标准见
[`../skills/search-gateway/references/answer-quality.md`](../skills/search-gateway/references/answer-quality.md)：
A 级要求每个数字、日期、统计量都有 URL 或 `[snippet: URL]`，且来源列表中的 URL 都出现在
本次工具结果里。

## 结论

| 维度 | 评级 | 说明 |
| --- | --- | --- |
| 检索执行 | **A-** | 15 次 `web_search`、8 次 `web_fetch`；5 路并行；无 `provider_errors`、无响应截断；延迟 1.3–9.1s。 |
| 证据可审计 | **C+** | 关键价格/ETF/央行数据有抓取页支撑；但多个数字来自摘要未标注，来源列表没有 URL，存在来源错配。 |
| Skill 触发 | **D** | 0 次 `Skill(search-gateway)`；模型只按全局提示词和工具描述执行。 |
| 分析质量 | **A-** | 校正 170t 口径、识别数据滞后、加入汇率渠道、概率分层、限制与置信度。 |
| **总评** | **B- / C+** | 能用，但未达到「每个数字都能点到来源」的审计标准。 |

## 工具执行数据

| 项 | 数量/结果 |
| --- | --- |
| `web_search` | 15 次（5 + 2 + 5 + 2 + 1） |
| `web_fetch` | 8 次 |
| `get_sources` | 0 次；所有搜索 `truncated=false`、`omitted_sources=0`，不需要分页 |
| `doctor` | 0 次；没有 provider 级失败，不需要诊断 |
| `provider_errors` | 0 |
| `chain_used` | `tavily` 或 `tavily+firecrawl` |
| 最长搜索 | 9.1s（上海金 11 月价格查询） |
| 最长抓取 | StoneX 17,606 字符，截到 12,000，`end_of_content=false` |

### 抓取质量

| 来源 | 质量分 | 提取器 | 状态 | 处理 |
| --- | --- | --- | --- | --- |
| 新浪财经（10-08 开盘价） | 70 | generic | 200 | 用于 10/8 开盘 890.5、9/30 收盘 907.32 |
| 世界黄金协会 ETF 月报 | 100 | generic | 200 | 用于 9 月 +$10bn、Q3 +$31bn、4,256t |
| Mitrade 行情分析 | 100 | generic | 200 | 用于 $3,948、10Y 4.90%、75% 加息概率 |
| StoneX 黄金预测 | 80 | firecrawl | 截断 | 用于 $4,000 失守、支撑位 |
| 世界黄金协会中国月报 | 85 | generic | 200 | **抓取结果以免责声明为主，未取到正文数据**；模型识别后改用 Al Jazeera |
| etf.com 资金流向 | 60 | firecrawl | **403 + warning** | 用于 GLD -$1.79bn、YTD -$3.83bn；未披露 403 |
| Al Jazeera 中国购金 | 100 | generic | 200 | 用于中国 9 月 +23t、储备 2,410t |
| 上海黄金交易所行情页 | 55 | firecrawl | 200 | 用于 Au99.99 845.5、最高 849.99、最低 834.0；低质量 fallback 未披露 |

## 达标项

- 独立子问题并行搜索，后续依赖问题顺序执行；`recency_days` 只用于时效性查询。
- 关键价格与 ETF/央行数据在引用前做了 `web_fetch`。
- 识别到世界黄金协会中国页面只返回免责声明，没有把该页当作正文证据。
- 识别到社交平台的单一低质量数字，最终答案没有把它写进主表。
- 最终答案纠正了「170t 是中国 YTD」的口径错误，加入 USD/CNY 渠道解释国内跌幅大于国际，
  给出 1–3 个月与 6–12 个月概率表、加权分、限制与置信度。
- 没有调用原生 `WebSearch` / `WebFetch`，没有切换到其他搜索 MCP。

## 未达标项

### 1. 引用层没有被执行

模型直接调用 MCP 工具，依据是全局提示词和工具 description。检索动作本身正确，但
source ledger、引用契约、审计步骤没有进入上下文。结果是「工具用对了，引用标准没执行」。

### 2. 来源列表没有 URL

最终答案以「主要来源：WGC《Central Bank Gold Statistics》；Kitco；Al Jazeera；……」结尾。
这是媒体名列表，不是可点击的来源列表。读者无法判断哪个数字对应哪一页。

### 3. 摘要被当成已读证据

多个数字来自 `web_search` 的 `description`，没有成功 `web_fetch`，最终答案没有标
`[snippet]`：国内收盘价、周成交量、水贝现象、DXY、核心 PCE、USD/CNY 预测、
12 月加息概率、投行目标价。

### 4. 来源错配

- 答案写「Sina Finance 2026-10-08/11-01」。10-08 的文章确实抓取了；11-01 的水贝文章
  来自另一个站点。
- 答案写「Reuters/TheStreet 2026-10-30」。75% 的实际来源是已抓取的 Mitrade；84% 来自
  搜索摘要中的转述；「TheStreet」没有抓取记录。
- 答案写「UBS/Yahoo Finance 2026-10-11」。2027 目标价的实际搜索结果是一条摘要，
  没有抓取 Yahoo Finance 或 UBS 原文。

### 5. 质量警告未披露

- `etf.com` 返回 403，`warning.code=http_4xx`，内容来自 Firecrawl 兜底；GLD 数字被直接
  使用，没有说明页面是 403 兜底内容。
- 上金所页面 `quality_score=55`，因通用提取质量 35 而走 Firecrawl 兜底；低质量标记没有
  出现在答案里。
- StoneX 抓取被截断在 12,000/17,606，`end_of_content=false`；答案使用了其中的支撑位，
  没有说明只读了前 12,000 字符。

### 6. 单一来源未标注

「限制与置信度」只列了数据缺口，没有逐条标注单一来源。至少收盘价、周成交量、水贝现象、
DXY、核心 PCE、汇率预测属于单一来源或摘要来源。

## 根因

1. **触发面偏窄**：skill description 主要命中显式检索词。本次请求是「分析走势、列概率」，
   没有检索动词，模型把它当成分析任务直接进入工具调用。
2. **提示词里的引用规则是「格式建议」而不是「硬门槛」**：没有写「数字没有 URL 就是未完成」，
   模型在长答案里退化成媒体名列表。
3. **工具返回了质量字段，但没有「引用就绪」判断**：`quality_score`、`warning`、
   `http_status` 都在响应里，模型也注意到了，但最终答案没有转成可见 caveat。
4. **来源列表没有和工具结果做一致性校验**：模型凭记忆写媒体名，导致错配。
5. **不能依赖 skill 自动触发**：本次 0 次调用。标准必须同时落在全局提示词和
   answer-quality 清单里。
6. **组装引用的摩擦高**：模型要自己从响应里拼 URL、标题、日期、质量；摩擦一大，
   就退回「媒体名 + 日期」的低成本写法。

## 已落地的改动（2026-11-03）

- `web_fetch` 增加 `citation_ready` 与 `citation_caveat`：质量分低于 45、源站 4xx、
  存在 `warning` 或页面截断时为 `false`，并给出可读原因；宿主必须披露 caveat 或换来源。
- `web_search.meta.snippet_only` 与 `get_sources.snippet_only` 恒为 `true`，显式提示
  「只有摘要，引用前必须 `web_fetch`」。
- skill 侧补充「检索前」与「引用契约」章节，并把「数字必须有 URL 或 `[snippet: URL]`」
  写成硬规则。
- 新增 4 个单元测试；完整测试数从 73 增至 77。

## 待做（0.3）

1. `web_fetch` 增加可直接复制的 `citation` 对象：`{url, title, fetched_at, http_status,
   quality, quality_score, extractor, warning}`，减少宿主自己拼引用的摩擦。
2. 可选：把答案里的 URL 与本次会话的搜索/抓取记录做一致性检查。先观察 `citation_ready`
   是否足够，再决定是否做。

## 结论

这套服务的**检索层**是可靠的：无 provider 错误、无截断、延迟可控。
**引用层**需要宿主模型配合，工具只能把「这页能不能引用」标出来，不能替模型写引用。
`citation_ready` 就是从这个失败模式里长出来的——它把「低质量页/4xx/截断」变成机器可判断的
字段，而不是靠模型自觉。
