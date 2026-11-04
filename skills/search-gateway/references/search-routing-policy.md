# Search gateway policy

The canonical search policy is [`agent-prompt.md`](agent-prompt.md). It contains the
global prompt sections §1 (Search and Evidence Standards) and §1b (Search gateway
workflow), including the citation contract and the pre-send audit.

This file is intentionally a pointer, not a second copy. Two policy files drift; one does
not.

## Adoption

1. Copy `skills/search-gateway/` into `~/.claude/skills/search-gateway/` (user scope) or a
   project's `.claude/skills/search-gateway/` (project scope).
2. Replace §1/§1b in `~/.claude/CLAUDE.md` with the corresponding sections from
   `agent-prompt.md`.
3. Keep the source-quality rules: two independent sources, explicit citation URLs,
   `[snippet: URL]` for snippet-only claims, `[单一来源]` for single-source claims,
   confidence levels, and no fabricated references.
4. Install, verify, and drift-check with `portable-setup.md`.
