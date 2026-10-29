#!/usr/bin/env bash
# Installs a custom Claude Code theme into ~/.claude/themes/ and points the
# global ~/.claude/settings.json "theme" key at it. Themes are a user-level
# (per-machine) setting, not a project-level one — see docs/Claude-Code-Setup.md's
# config layer split — so this always writes to $HOME, regardless of where the
# script itself is run from.
#
# Usage: scripts/apply-theme.sh [theme-name]   (default: linear-dark)
# Requires: jq

set -euo pipefail

THEME_NAME="${1:-linear-dark}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_THEME="$SCRIPT_DIR/../themes/${THEME_NAME}.json"
CLAUDE_DIR="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
DEST_THEME="$CLAUDE_DIR/themes/${THEME_NAME}.json"
SETTINGS="$CLAUDE_DIR/settings.json"

if [ ! -f "$SRC_THEME" ]; then
  echo "apply-theme: no theme file at $SRC_THEME" >&2
  exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "apply-theme: jq is required (brew install jq / apt install jq)" >&2
  exit 1
fi

mkdir -p "$CLAUDE_DIR/themes"
cp "$SRC_THEME" "$DEST_THEME"
echo "apply-theme: installed $DEST_THEME"

if [ -f "$SETTINGS" ]; then
  tmp="$(mktemp)"
  jq --arg theme "custom:${THEME_NAME}" '.theme = $theme' "$SETTINGS" > "$tmp"
  mv "$tmp" "$SETTINGS"
else
  mkdir -p "$CLAUDE_DIR"
  jq -n --arg theme "custom:${THEME_NAME}" '{theme: $theme}' > "$SETTINGS"
fi
echo "apply-theme: set \"theme\": \"custom:${THEME_NAME}\" in $SETTINGS"
echo "apply-theme: restart Claude Code (or /theme reload if supported) to see it"
