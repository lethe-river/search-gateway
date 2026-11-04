# search-gateway 验证复盘（会话 c4d167fe）

- 日期：2026-11-03
- 对象：`search-gateway` MCP + `search-gateway` skill + 全局提示词 §1b
- 样本：黄金趋势与国内价格分析。用户粘贴 4 个指标（央行购金、ETF、美元/10Y、上金所
  Au99.99），要求分析后续涨跌并列出概率。
- 模型：`clinepass/cline-pass/deepseek-v4.1-flash-1017[1m]`
- 转录：Claude Code 本地会话 JSONL（未提交）

## 结论

| 维度 | 评级 | 说明 |
| --- | --- | --- |
| 检索执行 | **A-** | 15 次 `web_search`、8 次 `web_fetch`；5 路并行；无 `provider_errors`、无响应截断；延迟 1.3–9.1s。 |
| 证据可审计 | **C+** | 关键价格/ETF/央行数据有抓取页支撑；但多个数字来自摘要未标注，来源列表没有 URL，存在来源错配。 |
| Skill 触发 | **D** | 0 次 `Skill(search-gateway)`；模型只按全局 §1b 和工具描述执行。 |
| 分析质量 | **A-** | 校正 170t 口径、识别数据滞后、加入汇率渠道、概率分层、限制与置信度。 |
| **总评** | **B- / C+** | 能用，但未达到“每个数字都能点到来源”的审计标准。 |

判定标准来自 `skills/search-gateway/references/answer-quality.md`：A 级要求每个数字、
日期、统计量都有 URL 或 `[snippet: URL]`，且来源列表中的 URL 都出现在本次工具结果里。

## 工具执行数据

| 项 | 数量/结果 |
| --- | --- |
| `ToolSearch` | 1 次，加载 `web_search` / `web_fetch` / `doctor` / `get_sources` |
| `web_search` | 15 次（5 + 2 + 5 + 2 + 1） |
| `web_fetch` | 8 次 |
| `get_sources` | 0 次；所有搜索 `truncated=false`、`omitted_sources=0`，不需要分页 |
| `doctor` | 0 次；没有 provider 级失败，不需要诊断 |
| `Skill(search-gateway)` | **0 次** |
| `provider_errors` | 0 |
| `chain_used` | `tavily` 或 `tavily+firecrawl` |
| 最长搜索 | 9.1s（上海金 11 月价格查询） |
| 最长抓取 | StoneX 17,606 字符，截到 12,000，`end_of_content=false` |

### 抓取质量

| URL | 质量 | 提取器 | 状态 | 处理 |
| --- | --- | --- | --- | --- |
| `finance.sina.cn/2026-10-08/...` | 70 | generic | 200 | 用于 10/8 开盘 890.5、9/30 收盘 907.32 |
| `gold.org/.../gold-etfs-holdings-and-flows/2026/10` | 100 | generic | 200 | 用于 9 月 +$10bn、Q3 +$31bn、4,256t |
| `mitrade.com/.../gold-oil-tumbles-diplomacy-midterms` | 100 | generic | 200 | 用于 $3,948、10Y 4.90%、75% 加息概率 |
| `stonex.com/.../gold-forecast-...` | 80 | firecrawl | 截断 | 用于 $4,000 失守、支撑位 |
| `gold.org/.../china-gold-market-update...` | 85 | generic | 200 | **抓取结果以免责声明为主，未取到正文数据**；模型在 thinking 中识别并改用 Al Jazeera |
| `etf.com/.../etfs-pull-204b-october...` | 60 | firecrawl | **403 + warning** | 用于 GLD -$1.79bn、YTD -$3.83bn；未披露 403 |
| `aljazeera.com/.../why-is-china-buying-up-gold` | 100 | generic | 200 | 用于中国 9 月 +23t、储备 2,410t |
| `sge.com.cn/sjzx/yshqbg` | 55 | firecrawl | 200 | 用于 Au99.99 845.5、最高 849.99、最低 834.0；低质量 fallback 未披露 |

## 达标项

- 独立子问题并行搜索，后续依赖问题顺序执行；`recency_days` 只用于时效性查询。
- 关键价格与 ETF/央行数据在引用前做了 `web_fetch`：Mitrade、StoneX、Sina、WGC ETF、
  Al Jazeera、SGE。
- 识别到 WGC China 页面只返回免责声明，没有把该页当作正文证据。
- 识别到 Facebook 的“10 月末周流出 38.3t”是单一低质量来源，最终答案没有把它写进主表。
- 最终答案纠正了“170t 是中国 YTD”的口径错误；指出 880 已破、量能条件未触发；加入
  USD/CNY 渠道解释国内跌幅大于国际；给出 1–3 个月与 6–12 个月概率表、加权分、限制与
  置信度。
- 没有调用原生 `WebSearch` / `WebFetch`，没有切换到其他搜索 MCP。
- 中文输出、`[P]` 前缀、Markdown 结构符合全局规范。

## 未达标项

### 1. Skill 未被加载

转录中 0 次 `Skill(search-gateway)`。模型直接调用 MCP 工具，依据是全局 §1b 和工具
description。检索动作本身正确，但 skill 里的 source ledger、引用契约、审计步骤没有进入
上下文。结果是“工具用对了，引用标准没执行”。

### 2. 来源列表没有 URL

最终答案以“主要来源：WGC《Central Bank Gold Statistics》2026-10-06；Kitco 2026-10-06；
Al Jazeera 2026-10-16；……”结尾。这是媒体名列表，不是可点击的来源列表。读者无法判断
哪个数字对应哪一页。

### 3. 摘要被当成已读证据

以下数字来自 `web_search` 的 `description`，没有成功 `web_fetch`，最终答案没有标
`[snippet]`：

- 10/30 Au99.99 收 854.76、夜盘 833（China.com 摘要）
- 上金所周成交量 +27.6%（Jin10 摘要）
- 水贝抢购/排队（China.com 摘要）
- DXY 101.8（Vantage 摘要）
- 核心 PCE 3.1%（搜索摘要；已抓取的 StoneX 只给了核心 PCE +0.3% m/m）
- USD/CNY 6.6531、MUFG 2027Q3 6.50（MUFG 摘要）
- 12 月加息 84%（Reuters 摘要；75% 来自已抓取的 Mitrade）
- 2027 投行目标 $5,000–5,400（goldtrack.io 摘要）

### 4. 来源错配

- 最终答案写 `Sina Finance 2026-10-08/11-01`。10-08 的 Sina 文章确实抓取了；11-01 的
  水贝文章来自 `3g.china.com`，不是 Sina。
- 最终答案写 `Reuters/TheStreet 2026-10-30`。75% 的实际来源是已抓取的 Mitrade；84%
  来自搜索摘要中的 Reuters 转述；“TheStreet”没有抓取记录。
- 最终答案写 `UBS/Yahoo Finance 2026-10-11`。2027 目标价的实际搜索结果是
  `goldtrack.io` 摘要，没有抓取 Yahoo Finance 或 UBS 原文。
- 最终答案写 `Kitco 2026-10-06` 和 `WGC《Central Bank Gold Statistics》2026-10-06`。
  两者都是搜索摘要，没有 `web_fetch` 记录。39t/170t 这一组关键数据因此没有“抓取页”
  支撑。

### 5. 质量警告未披露

- `etf.com` 返回 403，`warning.code=http_4xx`，内容来自 Firecrawl 兜底；GLD 数字被直接
  使用，没有说明页面是 403 兜底内容。
- `sge.com.cn` 的 `quality_score=55`，Firecrawl 因通用提取质量 35 而兜底；虽然表格
  确实抓到了 845.5，但低质量标记没有出现在答案里。
- StoneX 抓取被截断在 12,000/17,606，`end_of_content=false`；答案使用了其中的支撑位，
  没有说明只读了前 12,000 字符。

### 6. 单一来源未标注

最终答案的“限制与置信度”只列了数据缺口（全球央行到 8 月、中国到 9 月、WGC 10 月 ETF
月报未出），没有逐条标注单一来源。至少 10/30 国内收盘、周成交量、水贝现象、DXY、
核心 PCE、USD/CNY 预测属于单一来源或摘要来源。

## 根因

1. **触发面偏窄**：skill description 主要命中“搜索、查资料、找来源、核实、抓取网页”等
   显式检索词。本次请求是“分析走势、列概率”，没有检索动词；模型把它当成分析任务，
   直接进入工具调用。
2. **全局提示词的引用规则是“格式建议”，不是“硬门槛”**：§1 写了 citation format，
   但没有写“数字没有 URL 就是未完成”。模型在长答案里退化成媒体名列表。
3. **工具返回了质量字段，但没有“引用就绪”判断**：`quality_score`、`warning`、
   `http_status` 都在响应里，模型在 thinking 中也注意到了 WGC 页面只有免责声明、
   etf.com 403，但最终答案没有把这些转成可见 caveat。
4. **来源列表没有和工具结果做一致性校验**：模型凭记忆写媒体名，导致 Sina/China.com、
   Reuters/TheStreet、UBS/Yahoo 这类错配。
5. **不能依赖 Skill 自动触发**：本次是第三方模型，0 次 Skill 调用。即使 description
   命中，自动触发也是概率行为；标准必须同时落在全局提示词和 answer-quality 清单里。
6. **组装引用的摩擦高**：模型要自己从响应里拼 URL、标题、日期、质量；摩擦一大，就退回
   “媒体名 + 日期”的低成本写法。

## 优化方案

### 已落地（本仓库，2026-11-03）

- `SKILL.md`：扩展 description 到行情、价格、趋势、新闻分析、数据分析；新增
  “Before you search”和“Citation contract”；新增第 7 步“Audit before sending”；
  本地环境细节移出主文件。
- `references/agent-prompt.md`：全局提示词 §1/§1b 的唯一来源；把“数字必须有 URL 或
  `[snippet: URL]`”写成硬规则。
- `references/answer-quality.md`：A–D 评分、8 项检查、本次失败模式、引用示例。
- `references/portable-setup.md`：跨机器安装、标准层 vs 本地层、漂移检查。
- `references/local-environment.example.md`：参考机器部署，明确“不是标准”。
- `config.example.toml`、`.claude/settings.json`、`scripts/install.ps1` /
  `install.sh`：无密钥模板、项目级权限、可移植安装。
- `README.md`：新增“Agent 集成（可移植）”章节。

### 已落地（2026-11-03 追加，产品层）

- `web_fetch` 增加 `citation_ready` 与 `citation_caveat`：质量分低于 45、源站 4xx、
  存在 `warning` 或页面截断时为 `false`，并给出可读原因；宿主必须披露 caveat 或换来源。
- `web_search.meta.snippet_only` 与 `get_sources.snippet_only` 恒为 `true`，显式提示
  “只有摘要，引用前必须 `web_fetch`”。
- 新增 4 个单元测试；完整测试数从 73 增至 77。

### 待做（search-gateway 0.3）

1. `web_fetch` 增加 `citation` 对象：
   `{url, title, fetched_at, http_status, quality, quality_score, extractor, warning}`，
   宿主可以直接复制，不必自己拼。
2. 可选：`search-gateway --audit answer.md --session <id>`，把答案里的 URL 与本次
   session 的搜索/抓取记录做一致性检查。先观察 `citation_ready` 是否足够，再决定是否做。

### 验证方法

用同一组问题重跑一次，按 `answer-quality.md` 打分：

- 目标 A 级：0 个无 URL/无 `[snippet]` 标记的数字；0 个错配来源；所有质量警告披露；
  末尾有 `检索记录`。
- 再跑 3 个不同来源类型的样本：官方 API 文档、GitHub issue、新闻/市场分析。每个样本
  都检查“抓取前引用”和“来源列表 URL 可追溯”。
- 如果 skill 仍未自动加载，但全局提示词 + answer-quality 能让答案达到 A 级，说明标准
  已经落在正确层；skill 只作为工具参考，不再是唯一执行路径。

## 验收标准

| # | 标准 | 本次 | 目标 |
| --- | --- | --- | --- |
| 1 | 数字/日期/统计量有 URL 或 `[snippet: URL]` | 部分 | 全部 |
| 2 | 来源列表是 URL，不是媒体名 | 否 | 是 |
| 3 | URL 都出现在本次工具结果中 | 部分 | 全部 |
| 4 | `quality_score < 45` / `warning` / 4xx 已披露 | 否 | 全部 |
| 5 | 关键论断两个独立来源或 `[单一来源]` | 部分 | 全部 |
| 6 | 末尾有 `检索记录` | 否 | 是 |
| 7 | 未调用原生搜索工具 | 是 | 是 |

## 附：本次答案的优点

这不是一次失败的检索。相反，工具层执行得比多数样本好：

- 5 路并行搜索覆盖价格、美元/美债、国内金价、央行、ETF；随后按缺口补搜汇率、10 月
  ETF、2027 目标价。
- 对 WGC China 页面提取失败做了正确判断，改用 Al Jazeera 的已抓取正文。
- 对“10 月末周流出 38.3t”这条 Facebook 单一来源做了取舍，没有写进主表。
- 把用户表里的“170t”纠正为全球 YTD，并区分中国 YTD 约 103t；这是本次分析最有价值的
  事实校正。
- 国内价格用 USD/CNY 折算，指出 845 附近基本平水，解释了国内跌幅大于国际的原因。
- 概率表给出基准情景、关键条件、验证点和置信度，符合“结论先、证据后、限制最后”的
  结构。

问题集中在最后一公里：引用没有落地成 URL，质量警告没有进入答案。这也是本轮优化把
“Citation Contract”和“Answer quality rubric”放在最前面的原因。

## 复测：会话 04dec015（2026-11-03）

- 样本：同一组黄金问题；模型 `clinepass/cline-pass/deepseek-v4.1-flash-1017`。
- 工具：`Skill(search-gateway)` 1 次、`doctor` 1 次、`web_search` 12 次、
  `web_fetch` 24 次、`get_sources` 0 次；12 次搜索均 `truncated=false`、
  `omitted_sources=0`，不需要分页。
- 引用：最终答案 20 个唯一 URL；17 个已抓取、2 个搜索摘要、1 个不在本次会话
  （Norada 域名拼写错误）。
- 评分：检索执行 **A**；证据可审计 **B-**；Skill 触发 **A**；分析质量 **A-**；
  总评 **B-**。

### 达标

- Skill 首次被加载；引用前抓取 24 页，来源列表全部是 URL。
- 3 处 snippet-only 明示 `[snippet: ...]`；401/402/403 抓取失败在局限中披露。
- 关键价格、央行、ETF、国内消费数据与抓取正文一致；170t 口径纠正、
  SGE“价跌量增”判断正确。
- 质量门字段已生效：24 次抓取中 8 次 `citation_ready=false`，工具正确给出
  `citation_caveat`。

### 仍未达标

1. **截断页未披露**：`gold.org` ETF、`NAGA`、`goldtrack` 的抓取均为
   `citation_ready=false`，`citation_caveat` 明确要求继续分页；答案仍直接引用。
2. **Norada URL 拼写错误**：答案写 `noradaarealestate.com`，搜索结果实际是
   `noradarealestate.com`；该 URL 不在本次工具结果中。
3. **Yahoo 与 $3,875 不匹配**：Yahoo 正文只有 `$3,975`；`$3,875.19` 实际在
   Sina 抓取页。
4. **0 个 `[单一来源]` 标记**：SGE 10.68 吨、Sina 上半年消费、Crux 45% 零售商、
   NAGA 概率、GoldTrack 银行目标、TE 模型预测均为单来源或单聚合来源。
5. **少量数字/日期缺少就地 URL**：`$3,500` 技术目标只有搜索摘要、无 URL；
   `$5,608.35/$5,598.75` 在局限段无链接；12 月 9 日 FOMC、11 月 6 日非农无来源链接。

### 结论

较上次 C+ 明显提升：Skill 触发、URL 来源列表、snippet 标注、抓取失败披露都已落地。
剩余问题集中在“引用门”执行：`citation_ready=false` 的截断页未被当作硬约束，以及
单来源未标注。修掉这 5 项后可达 A-/A。

### 本轮修复（2026-11-03）

- `SKILL.md`：Citation contract 增加硬性 **Citation gate**、**URL exactness**、
  **Dates**；第 4 步要求截断页续读到 `end_of_content=true`；第 5 步要求
  `[单一来源]` 贴在论断旁；第 7 步增加机械审计；答案模板增加 `citation_caveat` 行。
- `references/agent-prompt.md` 与全局 `CLAUDE.md`：同步硬门规则——截断页必须续读或
  逐条 caveat，URL 不得重打，日期必须有来源，单来源标记必须贴着论断。
- `src/mcp.rs` 工具描述：`web_fetch` 明确 `citation_ready=false` 是硬引用门；
  `web_search` / `get_sources` 明确 `snippet_only` 必须先 `web_fetch` 再引用。
- `references/answer-quality.md`：新增失败模式 9–13（错链、截断当完整、来源-数字
  错配、单来源未标、日期无源），检查表增加第 8 项 URL 精确性。
- `README.md` 与 ADR 0005：同步截断续读与 caveat 规则。
- 验证：`cargo test` 77 项通过（46 单元 + 29 集成 + 2 MCP smoke）；新 release
  二进制已替换，`--doctor` 退出码 0；skill 已重新安装并与仓库 `diff -r` 一致。

## 复测：会话 330cc44a（2026-11-03）

- 样本：同一组黄金问题；模型 `clinepass/cline-pass/deepseek-v4.1-flash-1017[1m]`。
- 工具：`Skill(search-gateway)` 1 次、`doctor` 1 次、`web_search` 15 次、
  `web_fetch` 18 次（含 3 次续读）、`get_sources` 0 次；15 次搜索均
  `truncated=false`、`omitted_sources=0`。
- 引用：最终答案 15 个唯一 URL；12 个已抓取、3 个仅摘要；15/15 与工具结果逐字
  一致；0 个错链。
- 评分：检索执行 **A**；证据可审计 **B+**；Skill 触发 **A**；分析质量 **A-**；
  总评 **B+**（按 A/B/C/D 四档为 B）。

### 8 项检查

| # | 检查 | 结果 | 证据 |
| --- | --- | --- | --- |
| 1 | 数字覆盖 | 部分 | 关键价格、ETF、央行、中国需求均有抓取页支撑；但 `7 月 23 吨` 只在搜索描述中，未标 `[snippet]`；`3,924` 也只在搜索描述中；信号表与观察表多行没有就地 URL；`11 月上旬`、`未来 1–2 周` 无来源。 |
| 2 | URL 溯源 | 通过 | 15/15 URL 出现在本次工具结果；12 个抓取、3 个仅摘要。 |
| 3 | 来源身份 | 部分 | `5,595`、`3,946.16`、`4.893`、`5.31`、`101.68`、`104.77`、`6.6545`、`836.08`、`92/234/43.03`、`1.8bn`、`5,400/4,650` 均与抓取正文一致；但 `7 月 23 吨` 不在所引 Kitco 页；`10.68 吨` 是 jrj 成交额/加权均价折算，原文只写 `1.07 万千克`，未展示公式。 |
| 4 | 抓取质量 | 通过（位置有瑕疵） | TheStreet 403 + 截断有就地 caveat；Morningstar 深度文、Morningstar/Dow Jones 403 在局限与检索记录披露；Marc 截断续读到 `end_of_content=true`；Reuters 401、Vantage 403 未采用并披露。 |
| 5 | 独立性 | 不通过 | 0 个 `[单一来源]`；大量关键数字是单域来源：`3,946.16`（i001）、`5,595/345t`（Morningstar）、`4.893/5.31`（Morningstar/Dow Jones）、`6.6545`（Marc）、`836.08`（jrj）、`100/310/4,256`（WGC）、GLD `1.8bn`（247wallst）、`92/234/43.03`（Jin10）、`5,000/5,400/4,650`（TheStreet）、`98/-56/-82`（Kitco）。 |
| 6 | 冲突 | 部分 | `170t` 与 `345t` 的口径差异已解释；`75%–85%` 作为区间呈现；但 Sina 的 `5,598.75` 高点与正文 `5,595` 未对照说明。 |
| 7 | 检索记录 | 通过 | 查询、提供方、12 个抓取 URL、3 个 snippet、401/403 未采用项均列出。 |
| 8 | URL 精确性 | 通过 | 15/15 逐字一致；无 Norada 式拼写错误；无重打链接。 |

### 达标

- 较 `04dec015` 的 20 个 URL 中 1 个拼写错误，本轮 15/15 全部落在会话结果里。
- 不再有“截断页无 caveat 直接引用”：TheStreet、Morningstar/Dow Jones 的截断与
  403 均在答案中披露；Marc 已续读到 `end_of_content=true`。
- 3 个仅摘要来源在正文或检索记录中标注了 `snippet`（格式不是严格的
  `[snippet: URL]`，但可审计）。
- 关键数字与抓取正文一致：`5,595`、`3,946.16`、`4.893/-15.1bp`、`101.68/-0.27%`、
  `104.77/-7.6%`、`6.6545`、`836.08`、`92/234/43.03`、`1.8bn/3.8bn`、
  `5,000/5,400/4,650` 均已逐项核对。
- `Skill(search-gateway)` 被加载，`doctor` 运行（本地检查全 ok，live 未验证）。

### 仍未达标

1. **0 个 `[单一来源]`**：这是本轮唯一硬失败项。多数关键数字只有一个独立域，
   按 contract 必须在论断旁标注。
2. **`7 月 23 吨` 是未标注的 snippet**：它只出现在 WGC/Kitco 的搜索描述里；
   所引 Kitco 抓取页只有 8 月 39 吨、YTD 170 吨，没有 7 月 23 吨。
3. **`3,924` 是未标注的 snippet**：正文用它计算 0.4% 贴水，但该数字只出现在
   pricegold.net 的搜索描述中。
4. **`10.68 吨` 的推导未展示**：jrj 原文只写 `1.07 万千克`；按成交额 89.37 亿、
   加权均价 837.21 折算约为 10.67–10.68 吨，应写出公式或直接用 `1.07 万千克`。
5. **表格就地 URL 不足**：信号表 4 行、观察表 3 行没有逐行来源；`11 月上旬`、
   `未来 1–2 周` 两个时间点无来源。
6. **`5,595` vs `5,598.75` 未对照**：Sina 抓取页写 `5,598.75` 高点，正文用
   Morningstar 的 `5,595` 计算 `-29.5%`；两者差异小，但按检查 6 应说明口径。
7. **403 caveat 的位置**：TheStreet 有就地 caveat，但 global Morningstar 与
   Morningstar/Dow Jones 的 caveat 只在“局限”和“检索记录”，没有贴到每处引用。

### 结论

检索层已经稳定：15 次搜索、18 次抓取、15/15 URL 精确、无错链、无未披露的 4xx。
剩余问题集中在引用审计的最后一公里：单来源未标注、两个 snippet 数字未标注、
一个折算数字未展示公式、一个高点口径未对照。修掉第 1–4 项可达 A-；第 5–7 项
影响 A 级完整性。

### 可移植层同步（2026-11-03）

- `references/portable-setup.md`：验收改为 8 项；PowerShell 漂移检查从无效的
  `diff -r` 改为 `git diff --no-index`；增加全局提示词漂移检查；配置路径改为
  OS 默认路径，当前目录 `config.toml` 仅在 `APPDATA` 和 `HOME` 都缺失时使用。
- `SKILL.md`：机械审计增加表格逐行来源、派生数字公式、literal `[snippet: URL]`、
  单来源默认标注；配置路径说明与 `src/config.rs:614-626` 对齐。
- `answer-quality.md`：新增失败模式 14（派生数字无公式）、15（表格行无来源）。
- `README.md`：Agent 集成增加“二进制 + skill + 全局提示词同版”与 8 项验收；
  配置路径说明同步修正。
