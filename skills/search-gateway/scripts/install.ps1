# Install the search-gateway skill into the user-level Claude Code skills directory.
# This script only copies files. It does not edit settings.json, register the MCP
# server, or touch API keys.
param(
    [switch]$Force,
    [string]$SkillsDir = ""
)

$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$Source = Join-Path $RepoRoot "skills\search-gateway"

if (-not (Test-Path -LiteralPath $Source)) {
    throw "Skill source not found: $Source"
}

if ([string]::IsNullOrWhiteSpace($SkillsDir)) {
    $SkillsDir = Join-Path $HOME ".claude\skills"
}

$Dest = Join-Path $SkillsDir "search-gateway"

if (Test-Path -LiteralPath $Dest) {
    if (-not $Force) {
        throw "Already installed: $Dest`nRe-run with -Force to replace it."
    }
    Remove-Item -LiteralPath $Dest -Recurse -Force
}

New-Item -ItemType Directory -Force -Path $SkillsDir | Out-Null
Copy-Item -LiteralPath $Source -Destination $Dest -Recurse -Force

Write-Host "Installed skill: $Dest"
Write-Host ""
Write-Host "Next steps:"
Write-Host "1. Copy sections 1 and 1b from:"
Write-Host "   $Dest\references\agent-prompt.md"
Write-Host "   into $HOME\.claude\CLAUDE.md (replace the old search sections)."
Write-Host "2. Register the MCP server (see references\portable-setup.md):"
Write-Host "   claude mcp add --scope user search-gateway -- <binary> --config <config.toml>"
Write-Host "3. Ensure $HOME\.claude\settings.json permissions.allow contains:"
Write-Host '   "mcp__search-gateway__*"'
Write-Host '   "Skill(search-gateway)"'
Write-Host "4. Verify:"
Write-Host "   <binary> --doctor --config <config.toml>"
Write-Host "   claude mcp get search-gateway"
