# SearXNG 兜底方案

- 状态：SearXNG 部署已迁移并验证；search-gateway 客户端待实现
- 日期：2026-11-02
- 适用：`search-gateway` 的 `web_search`
- 相关决策：ADR 0003（最后一级兜底）、ADR 0004（compose 归属）

## 1. 目标与边界

SearXNG 是搜索链的最后一级（Last Resort），只在 Tavily 与 Firecrawl 都失败或归一化后零来源时启用。

- **不新增容器**：复用现有 `searxng` Compose 项目，并将其定义迁入本仓库 `deploy/searxng/`（ADR 0004）。`search-gateway` 本身是 Rust + stdio 本地进程，不占容器。
- **不参与补位**：Tavily 结果不足由 Firecrawl 补足；SearXNG 只在全部主力失败/零结果时启动。
- **只返回来源**：不调用模型、不生成 answer；输出同构 `Source`，`provider = "searxng"`。
- **只负责搜索**：`web_fetch` / `web_map` 不经过 SearXNG；抓取链保持“专用解析器 → 通用 HTTP → Firecrawl”。
- **只返回摘要**：SearXNG 的 `content` 是 snippet，写入 `Source.description`；正文由宿主按需走抓取链。

## 2. 现状实测（2026-11-02）

实例：`searxng-core`，镜像 `2026.8.4-c63835bd2`，`127.0.0.1:12310`，`/healthz` 返回 200（6ms）。当前 `use_default_settings: true`，83 个引擎启用。

| 引擎 | 当前状态 | 实测 | 结论 |
|---|---|---|---|
| google cse | 启用 | 4/4 查询各 20 条，0.52–2.54s | 当前唯一稳定通用引擎 |
| brave | 启用 | 429 / Suspended | 关闭 |
| duckduckgo | 启用 | CAPTCHA（HTTP 202） | 关闭 |
| startpage | 启用 | Suspended: CAPTCHA | 关闭 |
| bing | 默认禁用 | 临时实例 12311：7 条；容器直连 `www.bing.com` 200 / 1.12s | 启用 |
| 360search | 默认禁用 | 临时实例：6 条；容器直连 `www.so.com` 200 / 0.21s | 启用，超时压到 6s |
| baidu / sogou / qwant / yandex / mojeek | 默认禁用 | 此前实测 CAPTCHA / 拒绝 / 无响应 | 保持关闭 |
| wikipedia / wikidata | 启用 | 参考类；本次 `engines=wikipedia` 探测 timeout | 保留，不依赖 |
| stackoverflow / arxiv | 启用 | 临时实例各 10 条 | 保留 |
| github | 启用 | 临时实例曾 unresponsive | 保留，不依赖 |

关键事实：

1. `google cse` 是 SearXNG 自带引擎，使用 blackle.com 的公共 CSE（`partner-pub-8993703457585266:4862972284`），不需要 API key，token 缓存 1 小时。能用，但它是第三方单点。
2. SearXNG 会等所有选中引擎返回或超时后才输出 JSON。`360search` 在默认 `settings.yml` 中是 `timeout: 20.0`；启用时必须覆盖为 6s，否则一个挂住的 360 可能把整次搜索拖到 20 秒。
3. `timeout_limit` 查询参数有效（实测 `timeout_limit=1` → 1.01s 返回）；`engines=` 支持逗号分隔，且能选中默认禁用的引擎。但本方案不硬编码引擎名，使用 `categories` + 服务端白名单，避免镜像升级后引擎改名导致 400。

## 3. 部署归属与迁移（方案 B）

现有部署：

- 目录：`<old-searxng-dir>`
- Compose 项目名：`searxng`
- 容器：`searxng-core`（127.0.0.1:12310）、`searxng-valkey`（仅内部）
- 命名卷：`searxng_core-data`、`searxng_valkey-data`

目标：迁入 `<repo>\deploy\searxng\`，仍是同一个项目、同一个容器、同一组命名卷，不新增容器项目。

迁移步骤（已执行，2026-11-02）：

```powershell
# 1. 先停旧项目（保留命名卷）
docker compose -f <old-searxng-dir>\docker-compose.yml down

# 2. 迁到 search-gateway 仓库
New-Item -ItemType Directory -Force <repo>\deploy\searxng
Move-Item <old-searxng-dir>\docker-compose.yml <repo>\deploy\searxng\
Move-Item <old-searxng-dir>\.env               <repo>\deploy\searxng\
Move-Item <old-searxng-dir>\core-config        <repo>\deploy\searxng\

# 3. 启动（文件里已有 name: searxng，命名卷不变）
docker compose -f <repo>\deploy\searxng\docker-compose.yml up -d

# 4. 验证
curl.exe -s -o NUL -w "%{http_code}" http://127.0.0.1:12310/healthz
```

约束：

- 必须保留 `name: searxng` 与 `container_name: searxng-core`；否则 Compose 会把它当新项目，创建新容器并占用 12310。
- 必须先 `down` 再 `up`；`down` 不带 `-v`，命名卷保留。
- `core-config` 是 bind mount（`./core-config/:/etc/searxng/`），必须随 compose 文件移动。
- `searxng-valkey` 当前未被 core 使用（`.env` 没有 `SEARXNG_VALKEY_URL`），可从 compose 删除；不影响搜索。
- 回滚：把三个路径移回 `<old-searxng-dir>`，再 `up -d`。

迁移后目录：

```text
search-gateway/
  deploy/searxng/
    docker-compose.yml
    .env
    core-config/settings.yml
```

## 4. SearXNG 侧配置

文件：`deploy/searxng/core-config/settings.yml`。保留现有 `secret_key`、`image_proxy`、`search.formats`，补齐：

```yaml
use_default_settings: true

general:
  instance_name: "SearXNG (local)"

server:
  secret_key: "<保持现有值，不要改>"
  image_proxy: true

search:
  formats:
    - html
    - json

outgoing:
  request_timeout: 6.0      # 单引擎默认超时（原 3.0）
  max_request_timeout: 8.0  # 整次搜索硬上限

engines:
  # 保留 / 启用：实测能出结果的通用引擎
  - name: google cse
    disabled: false
  - name: bing
    disabled: false
  - name: 360search
    disabled: false
    timeout: 6.0            # 默认 20s，必须覆盖

  # 实测不可用：关闭，避免重启后的首个请求等它们超时
  - name: brave
    disabled: true
  - name: duckduckgo
    disabled: true
  - name: startpage
    disabled: true
```

说明：

- `request_timeout: 6.0` 是单引擎默认超时；`max_request_timeout: 8.0` 是整次搜索硬上限。
- `360search` 默认条目是 `timeout: 20.0`，必须显式覆盖。
- `brave` / `duckduckgo` / `startpage` 实测 429 / CAPTCHA，关闭；否则重启后首个请求会等它们超时。
- `baidu` / `sogou` / `qwant` / `yandex` / `mojeek` 默认已禁用，不必重复写。
- `stackoverflow` / `arxiv` / `github` / `wikipedia` / `wikidata` 默认已启用，保持不动，由网关按 `categories` 选用。

## 5. `search-gateway` 客户端契约

配置：

```toml
[searxng]
enabled = true
base_url = "http://127.0.0.1:12310"
connect_timeout_ms = 2000
request_timeout_ms = 8000     # HTTP 客户端超时，需大于服务端 timeout_limit
server_timeout_s = 6          # 作为 timeout_limit 传给 SearXNG
categories = ["general"]      # 代码查询加 it，学术查询加 science
language = "auto"             # 含 CJK 用 zh-CN，否则 en；可强制 all
max_results = 10
breaker_failures = 3
breaker_cooldown_secs = 300
```

请求映射：

- `GET {base_url}/search`
- 基础参数：`q`、`format=json`、`pageno=1`、`safesearch=0`、`timeout_limit=6`
- `categories`：默认 `general`；代码类加 `it`；学术类加 `science`
- `language`：CJK → `zh-CN`，否则 `en`；可配置 `all`
- `time_range`：`recency_days` ≤1 → `day`，≤7 → `week`，≤31 → `month`，≤365 → `year`，更大省略
- 域名：单个 include 可 pushdown 成 `site:domain`；多个 include 不拼多个 `site:`（AND 语义），统一本地过滤；exclude 也在本地过滤

响应映射：

```text
results[] -> Source {
  url:            result.url,
  provider:       "searxng",
  title:          result.title,
  description:    result.content,
  published_date: result.publishedDate ?? result.pubdate ?? null,
  content:        null
}
```

规则：

- 丢弃非 `http/https`、缺 `url` 或 `title` 的条目。
- 按规范化 URL 去重（小写 host、去掉 `utm_*` / `fbclid`、去尾斜杠），保留首条。
- 截断到 `max_results`（默认 10）。
- `unresponsive_engines` 只记日志；只要 `results > 0`，不判整次失败。
- 0 条结果不判熔断失败，记录 `zero_results` 指标。
- HTTP 429 / 5xx / 连接失败 / 超时：计熔断失败，不重试。
- HTTP 400 / 403：配置错误，熔断 600s，doctor 暴露。

## 6. 路由、熔断与时间预算

```text
web_search
  ├─ Tavily（keyring 轮询 + 冷却）       主提供方
  ├─ Firecrawl（rotator 轮询）           补位 / 故障转移
  └─ SearXNG（127.0.0.1:12310）          兜底（Last Resort）
```

- 触发：Tavily 与 Firecrawl 都失败，或归一化后来源数为 0。
- 不参与 top-up。
- 熔断：连续 3 次瞬态失败 → open 300s；期间直接跳过；冷却后 half-open 放 1 个探测；成功关闭，失败重开。
- 时间预算：整体 `web_search` 30s；Tavily 10s、Firecrawl 10s、SearXNG 8s；剩余预算 < 8s 时直接 `budget_exhausted`。
- SearXNG 服务端 `timeout_limit=6`，客户端 8s。

## 7. doctor

1. `GET /healthz` → 200，记录延迟。
2. `GET /config` → 检查 `google cse` / `bing` / `360search` 启用，`brave` / `duckduckgo` / `startpage` 禁用；不一致给 warning。
3. `GET /search?q=searxng+doctor&format=json&categories=general&timeout_limit=5` → `results > 0`；输出结果数、引擎分布、`unresponsive_engines`、延迟。
4. 输出熔断状态与最近一次错误。

失败分类：`unreachable`、`misconfigured`、`zero_results`。

## 8. 验收

- **黄金集**：10 条查询（中英文、通用/代码/学术各覆盖），禁用 Tavily/Firecrawl，要求每条至少 3 条来源，p95 < 8s，`Source` 字段完整。
- **故障注入**：停掉 `searxng-core` → 连续 3 次调用后熔断打开，搜索返回明确的 `provider_errors.searxng`，不挂起；重启后冷却期结束 half-open 恢复。
- **回归**：Tavily/Firecrawl 正常时不调用 SearXNG（日志断言），避免兜底被误触发。

## 9. 风险与边界

- SearXNG 无 SLA，引擎随时可能改版或限流；它只能做最后一级，不能当主力。
- `google cse` 依赖 blackle.com 的公共 CX 和 Google token 端点，是第三方单点；bing/360 是冗余，不是保证。
- `360search` 中文偏置明显；语言和 `categories` 路由会影响结果质量，需要黄金集校准。
- SearXNG 只给 snippet；宿主需要正文时，仍要走抓取链，兜底不会自动抓正文。
- 360 挂住时，整次请求仍会等到 `timeout_limit`（6s）才返回；这是 SearXNG 的聚合模式，不是 bug。
- 12310 是单容器；Docker/WSL 不在时熔断打开，doctor 报 `unreachable`。

## 10. 实施状态

1. 迁移 compose（方案 B）。已完成（2026-11-02）。
2. 应用 `settings.yml`。已完成（2026-11-02）。
3. 重启并跑探针。已完成（2026-11-02）。
4. 实现 `[searxng]` 客户端 + doctor。待实现。
5. 黄金集 + 故障注入。待实现。

## 11. 实施记录（2026-11-02）

- 已迁入 `deploy/searxng/`；Compose 项目名 `searxng`、容器名 `searxng-core` / `searxng-valkey`、命名卷 `searxng_core-data` 保持不变。
- `/config` 确认：`google cse` / `bing` / `360search` enabled，`brave` / `duckduckgo` / `startpage` disabled；三者的有效 timeout 均为 6.0s。
- `/healthz` 200（约 9ms）。
- 实测：
  - `rust programming` → 36 条（google cse 20 / bing 10 / 360search 6），`unresponsive` 为空。
  - `北京 天气预报` → 34 条（同样的三个引擎），`unresponsive` 为空。
  - `engines=bing` 单引擎 → 10 条。
- `searxng-valkey` 仍在运行但未被 core 使用；可选删除。
- 备份：`<old-searxng-dir>.bak-20261102`。
- 旧目录 `<old-searxng-dir>` 已空，未删除。
