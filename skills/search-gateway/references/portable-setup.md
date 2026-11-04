# 可移植安装与一致性标准

目标：在另一台电脑、本机另一个 agent、或另一个 Claude Code profile 上，使用同一套
search-gateway 检索与引用标准。标准文件来自仓库；机器相关的路径、端口、key 只放在本地
配置里。

## 1. 哪些是标准，哪些是本地层

| 层 | 文件 | 是否跨机器一致 |
| --- | --- | --- |
| 检索/引用工作流 | `skills/search-gateway/SKILL.md` | 是 |
| 全局提示词 §1/§1b | `skills/search-gateway/references/agent-prompt.md` | 是 |
| 引用审计清单 | `skills/search-gateway/references/answer-quality.md` | 是 |
| 工具权限 | `.claude/settings.json` + 用户 `settings.json` | 是（权限项） |
| MCP 注册 | `claude mcp add ...` | 是（命令形状） |
| 发布组合 | release 二进制 + skill + 全局提示词 §1/§1b | 是（同一版） |
| 可执行文件路径 | `target/release/search-gateway` | 否（每台机器不同） |
| 配置文件路径 | `%APPDATA%` / `~/.config` / `--config` | 否（按 OS） |
| API keys、SearXNG 端口、代理网段 | 本地 `config.toml` | 否 |

**规则**：标准层只改仓库，不改 `~/.claude/skills` 里的副本。改完仓库后重新安装
（`install.ps1 -Force` / `install.sh --force`），不要在两处分别手改。Windows 上不要用
symlink（本机未开启开发者模式，`ln -s` 会静默变成复制）；实体目录 + 重新复制是支持路径。

## 2. 前置条件

- Rust stable。Windows 参考构建需要 GNU 目标与 MinGW-w64 binutils（`dlltool`、`as`）；
  Linux/macOS 用本机 stable 即可。
- Claude Code（CLI、桌面版或 IDE 扩展均可）。
- Tavily 和/或 Firecrawl API key；或一个可用的 SearXNG 实例。只有 SearXNG 时搜索质量
  和覆盖会下降，但零 API 费用可用。
- 网络能访问 `api.tavily.com` / `api.firecrawl.dev`，或你的自建代理。

## 3. 构建

Windows（仓库默认）：

```powershell
cargo build --release
cargo test
```

Linux/macOS：仓库的 `rust-toolchain.toml` 固定了 Windows GNU 目标，**不要照抄到
Linux/macOS**。用本机 stable 覆盖：

```bash
cargo +stable build --release
cargo +stable test
```

或者在 Linux/macOS 的 checkout 里删除/改写 `rust-toolchain.toml`。构建产物：

- Windows: `target\release\search-gateway.exe`
- Linux/macOS: `target/release/search-gateway`

如果目标机器不能编译，只复制同 OS/同架构的 release 二进制也可以；不要跨 OS 复制 exe。

## 4. 配置

配置路径优先级：

1. `--config PATH`
2. 环境变量 `SEARCH_GATEWAY_CONFIG`
3. OS 默认路径：Windows `%APPDATA%\search-gateway\config.toml`；Linux/macOS（有 `HOME` 时）
   `~/.config/search-gateway/config.toml`
4. 仅当 `APPDATA` 和 `HOME` 都未设置时，才使用当前目录 `config.toml`
5. 所选文件不存在时，服务模式报错；`--doctor` / `--selftest` 使用内置默认值并给出警告

当前目录的 `config.toml` **不是** OS 默认路径之后的兜底。要使用仓库内或项目内的配置，
必须显式传 `--config`，或设置 `SEARCH_GATEWAY_CONFIG`。

从仓库的 `config.example.toml` 复制一份，填入自己的 key。示例用
`env:TAVILY_API_KEY` / `env:FIRECRAWL_API_KEY`，不把密钥写进文件；也可以在 `keys`
数组里写多个 `env:NAME` 或字面值。一个环境变量可以是逗号/分号/空白分隔的多 key。

最小可用配置（只有 Tavily）：

```toml
[tavily]
enabled = true
base_url = "https://api.tavily.com"
keys = ["env:TAVILY_API_KEY"]
search_depth = "basic"
timeout_ms = 10000

[firecrawl]
enabled = false

[searxng]
enabled = false
```

只有 SearXNG 时：

```toml
[tavily]
enabled = false

[firecrawl]
enabled = false

[searxng]
enabled = true
base_url = "http://127.0.0.1:12310"
```

本机的 SearXNG 端口、Firecrawl rotator 版本、Clash fake-IP 例外等，写在
`references/local-environment.example.md` 里供参考；另一台机器不要继承这些值。

## 5. 注册 MCP

Windows PowerShell：

```powershell
claude mcp add --scope user search-gateway -- `
  "<repo>\target\release\search-gateway.exe" `
  --config "$env:APPDATA\search-gateway\config.toml"
```

Linux/macOS：

```bash
claude mcp add --scope user search-gateway -- \
  "$HOME/tools/search-gateway/target/release/search-gateway" \
  --config "$HOME/.config/search-gateway/config.toml"
```

验证：

```bash
claude mcp get search-gateway
claude mcp list
```

`claude mcp list` 里应显示 `search-gateway: ... - ✔ Connected`。如果连接失败，先直接运行
`search-gateway --doctor --config <path>`，看是配置、key 还是网络问题。

## 6. 安装 skill

Windows：

```powershell
.\skills\search-gateway\scripts\install.ps1 -Force
```

Linux/macOS：

```bash
./skills/search-gateway/scripts/install.sh --force
```

默认安装到 `~/.claude/skills/search-gateway`（可用 `CLAUDE_SKILLS_DIR` 覆盖）。安装后确认：

```text
~/.claude/skills/search-gateway/SKILL.md
~/.claude/skills/search-gateway/references/agent-prompt.md
~/.claude/skills/search-gateway/references/answer-quality.md
~/.claude/skills/search-gateway/references/portable-setup.md
~/.claude/skills/search-gateway/references/search-routing-policy.md
~/.claude/skills/search-gateway/references/local-environment.example.md
```

`search-routing-policy.md` 只是指向 `agent-prompt.md` 的指针，不是第二份策略；不要在两处
分别维护规则。

项目级使用：把 `skills/search-gateway/` 复制到项目的 `.claude/skills/search-gateway/`，
不要复制到用户目录。项目级 skill 只在该项目生效。

## 7. 全局提示词

把 `references/agent-prompt.md` 的 §1 和 §1b 复制到 `~/.claude/CLAUDE.md`，替换同名的
旧章节。`agent-prompt.md` 是唯一来源；不要在全局文件里再手写一份不同的规则。

保留全局文件里的其他个人章节（语言、reverse-skill 路由、第三方模型限制等）。如果全局
文件里还路由到别的搜索 MCP 或原生 `WebSearch` / `WebFetch`，删掉那些路由，否则会出现
两个标准。

如果机器上仍注册了其他搜索 MCP（例如 `grok-search-rs`），模型仍可能调用它们；只改提示词
不够。要强制执行单工具标准，用 `claude mcp remove <name> -s user` 移除，或在宿主里禁用
这些工具。移除前确认你没有其他流程依赖它。

## 8. 权限

用户级 `~/.claude/settings.json` 的 `permissions.allow` 至少包含：

```json
{
  "permissions": {
    "allow": [
      "mcp__search-gateway__*",
      "Skill(search-gateway)"
    ]
  }
}
```

项目级 `.claude/settings.json` 已包含同样两项，提交到仓库后，项目内自动生效。不要把这些
规则写进 `.claude/settings.local.json`（本机全局 gitignore 会忽略它，另一台机器拿不到）。

如果不想自动批准全部 search-gateway 工具，把 `mcp__search-gateway__*` 换成逐个工具：

```json
"mcp__search-gateway__web_search",
"mcp__search-gateway__web_fetch",
"mcp__search-gateway__web_map",
"mcp__search-gateway__get_sources",
"mcp__search-gateway__doctor"
```

## 9. 验证

```bash
# 不消耗付费额度；live_verified=false 是预期
search-gateway --doctor --config <path>

# 各消耗一个 Tavily/Firecrawl 请求，并做一次免费抓取探测
search-gateway --doctor --live --config <path>
```

然后在 Claude Code 里：

1. `/mcp` 确认 search-gateway 已连接。
2. 问一个需要外部数字的问题，例如“查一下某库最新版本的发布日期，并给出 URL”。
3. 按 `answer-quality.md` 的 8 项检查验收，不要只看“有没有来源列表”：
   1) 每个外部数字/日期有 URL 或 `[snippet: URL]`；
   2) 每个 URL 都出现在本次工具结果中；
   3) 来源名与 URL 一致，页面确实包含该数字；
   4) `citation_ready=false`、`quality_score < 45`、4xx/warning 已披露；
   5) 关键论断有两个独立来源或就地 `[单一来源]`；
   6) 冲突数字有权威/日期对照，或声明未解决；
   7) 末尾有 `检索记录`；
   8) URL 逐字复制，没有重打。
   其中 1、2、3、4、5、8 是硬门；任何一项不过，不算 A 级。
4. 如果 skill 没有被自动加载，但工具被正确调用，说明全局提示词在工作、skill 触发词不够；
   先按 `answer-quality.md` 手工审计，再考虑收窄/扩展 skill 的 description。

## 10. 更新与漂移检查

```powershell
# Windows
git pull
cargo build --release
.\skills\search-gateway\scripts\install.ps1 -Force
```

```bash
# Linux/macOS
git pull
cargo +stable build --release
./skills/search-gateway/scripts/install.sh --force
```

安装后可以对比仓库和用户目录，确认没有本地手改：

```powershell
git diff --no-index -- .\skills\search-gateway "$env:USERPROFILE\.claude\skills\search-gateway"
```

```bash
diff -r ./skills/search-gateway "$HOME/.claude/skills/search-gateway"
```

`git diff --no-index` 在存在差异时退出码为 1，这是预期；输出为空才表示无漂移。差异只应
出现在 `references/local-environment.md`（如果你本地建了这个文件）。其他差异说明有人
直接改了安装副本，应回到仓库修，再重新安装。

全局提示词也要单独检查，不能只看 skill 目录：

```powershell
Select-String -Path "$HOME\.claude\CLAUDE.md" -Pattern 'citation_ready=false','\[单一来源\]','\[snippet: URL\]'
```

```bash
grep -nE 'citation_ready=false|\[单一来源\]|\[snippet: URL\]' "$HOME/.claude/CLAUDE.md"
```

三处都命中，才说明 §1/§1b 包含当前硬规则；否则从 `agent-prompt.md` 重新复制，不要手工
补一句。

## 11. 故障排查

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| `claude mcp list` 显示 failed | 路径错、配置无效、key 缺失 | 直接运行二进制 + `--doctor --config` |
| `web_search` 全部 `auth` | key 无效或未注入 | 检查 `env:NAME` 是否在启动 Claude Code 的环境里 |
| `web_search` 全部 `rate_limited` | 多 agent 共用 key | 拆 key、降低并发，或等待 `retry_after_secs` |
| `web_fetch` 全部 `blocked` | 目标是 loopback/私网，或代理 fake-IP 未放行 | 不要绕过；确需本机页面时设 `allow_private_hosts=true` |
| Firecrawl 抓取失败但 Tavily 正常 | 本地 rotator 只支持 v1，服务调用 v2 | `base_url` 指向 `https://api.firecrawl.dev`，或升级 rotator |
| skill 不触发 | 请求是分析型，description 未命中 | 全局提示词已要求先加载；仍不触发时手工 `/search-gateway` 或补触发词 |
| 回答没有 URL | 模型把 citation 当风格建议 | 按 `answer-quality.md` 审计；全局提示词的 Citation Contract 是硬规则 |

## 12. 其他 MCP 宿主（非 Claude Code）

同一套标准可以给本机的其他 agent 使用，不需要为每个宿主重写检索逻辑：

1. 把 `search-gateway` 注册为 stdio MCP server，命令与 `claude mcp add` 里的
   `<binary> --config <config>` 相同。
2. 把 `agent-prompt.md` 的 §1/§1b 放进该宿主的系统提示词。宿主没有 `CLAUDE.md` 时，
   直接作为 system prompt 的一部分。
3. 如果宿主不支持 skill，把 `SKILL.md` 作为检索手册附在系统提示词或知识库里；至少保留
   “Citation contract”和“Audit before sending”两节。
4. 宿主没有 `Skill(search-gateway)` 权限概念；只需允许 5 个工具：
   `web_search`、`web_fetch`、`web_map`、`get_sources`、`doctor`。
5. 用同一套验收：`doctor` 通过；一个数字问题；答案达到 `answer-quality.md` 的 A 级。
   不要因为宿主不同就降低引用标准。
