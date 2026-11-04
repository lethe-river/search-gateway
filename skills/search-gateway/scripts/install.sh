#!/usr/bin/env sh
# Install the search-gateway skill into the user-level Claude Code skills directory.
# This script only copies files. It does not edit settings.json, register the MCP
# server, or touch API keys.
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../../.." && pwd)
SOURCE="$REPO_ROOT/skills/search-gateway"

if [ ! -d "$SOURCE" ]; then
    echo "Skill source not found: $SOURCE" >&2
    exit 1
fi

SKILLS_DIR="${CLAUDE_SKILLS_DIR:-$HOME/.claude/skills}"
DEST="$SKILLS_DIR/search-gateway"
FORCE=0

if [ "${1:-}" = "--force" ]; then
    FORCE=1
fi

if [ -e "$DEST" ]; then
    if [ "$FORCE" -ne 1 ]; then
        echo "Already installed: $DEST" >&2
        echo "Re-run with --force to replace it." >&2
        exit 1
    fi
    if [ -z "$DEST" ] || [ "$DEST" = "/" ]; then
        echo "Refusing to remove unsafe path: $DEST" >&2
        exit 1
    fi
    rm -rf "$DEST"
fi

mkdir -p "$SKILLS_DIR"
cp -R "$SOURCE" "$DEST"

echo "Installed skill: $DEST"
echo ""
echo "Next steps:"
echo "1. Copy sections 1 and 1b from:"
echo "   $DEST/references/agent-prompt.md"
echo "   into $HOME/.claude/CLAUDE.md (replace the old search sections)."
echo "2. Register the MCP server (see references/portable-setup.md):"
echo "   claude mcp add --scope user search-gateway -- <binary> --config <config.toml>"
echo "3. Ensure $HOME/.claude/settings.json permissions.allow contains:"
echo '   "mcp__search-gateway__*"'
echo '   "Skill(search-gateway)"'
echo "4. Verify:"
echo "   <binary> --doctor --config <config.toml>"
echo "   claude mcp get search-gateway"
