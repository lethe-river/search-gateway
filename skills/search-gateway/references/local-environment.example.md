# Local environment — reference machine example

This file documents the machine where the skill was developed. It is **an example, not part
of the portable standard**. On another machine, create your own
`references/local-environment.md` (or edit this file in your private checkout) and do not
inherit these paths, ports, or key sources.

## Deployment

- OS: Windows 11, GNU Rust target.
- Repository: `<repo>`.
- Binary: `<repo>\target\release\search-gateway.exe`.
- Config: `%APPDATA%\search-gateway\config.toml` (not committed; `/config.toml` is ignored).
- MCP registration: user scope, stdio transport, registered with `claude mcp add --scope user`.
- Skill: copied to `%USERPROFILE%\.claude\skills\search-gateway\` (no symlink; Windows
  developer mode is off, so `ln -s` silently copies).

## Providers

- Tavily: enabled, multiple keys, `base_url = "https://api.tavily.com"`.
- Firecrawl: enabled, multiple keys, `base_url = "https://api.firecrawl.dev"`.
- SearXNG: last resort, `base_url = "http://127.0.0.1:12310"`, deployed from
  `deploy/searxng/docker-compose.yml`.

## Firecrawl rotator caveat

The local `firecrawl-rotator` at `http://127.0.0.1:13000` only forwards `/v1/scrape` and
`/v1/search`. search-gateway calls `/v2/search`, `/v2/scrape`, and `/v2/map`. Do **not**
point `[firecrawl].base_url` at the rotator unless it is upgraded to v2. The reference
machine uses `https://api.firecrawl.dev` with the rotator's key pool copied into the config.

The rotator's key pool can change; re-import its `FIRECRAWL_API_KEYS` into the
search-gateway config when it does.

## Network

- Clash uses `198.18.0.0/15` for fake-IP. `web_fetch` allows that range; otherwise every
  public domain resolves into a blocked private address.
- `web_fetch` blocks loopback, private, link-local, multicast, and reserved addresses by
  default. `[fetch].allow_private_hosts = true` is only for a deliberate local-page fetch.
- Rate limit: 60 outbound search/fetch/map calls per minute by default (`[rate]`).

## Cache and persistence

- Query cache: in-process LRU, 600 seconds, 32 entries.
- Source sets: `sessions.json` next to the config file; `search_id` survives restarts until
  TTL expiry.
- Page cache: 600 seconds, 64 pages; `fetch_id` is **not** persistent across restarts.
- `get_sources` does not spend provider credits.
- `doctor` without `live` does not spend Tavily/Firecrawl credits; it may probe SearXNG.

## Why this file exists

The skill must not hard-code `%APPDATA%`, `127.0.0.1:12310`, or the rotator rule. Those are
facts about one deployment. Keeping them here means the standard in `SKILL.md` and
`agent-prompt.md` can be copied to another machine unchanged, while the local layer stays
replaceable.
