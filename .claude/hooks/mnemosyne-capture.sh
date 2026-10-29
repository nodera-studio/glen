#!/usr/bin/env bash
# Stop hook — memory MCP episodic-fix capture (Sink B).
#
# On end-of-turn (Claude Code's `Stop` event — there is no SessionEnd event),
# records one `episodic_fix` rule_findings row per changed source area, so the
# nightly distiller has raw evidence of what the agent actually touched.
#
# Posture (mirrors prettier-on-edit.sh — warn-don't-block):
#   - ALWAYS exit 0. Capture must NEVER gate the turn. It runs BEFORE any
#     blocking Stop-hook test gate you may add to the Stop array, and its
#     exit 0 can't suppress that gate's exit 2.
#   - Env-gated by MNEMOSYNE_CAPTURE=1 (off by default) so CI / other contexts'
#     Stop hooks are inert.
#   - Guards stop_hook_active so a stop-hook continuation doesn't re-capture.
set -uo pipefail

# Opt-in only.
[ "${MNEMOSYNE_CAPTURE:-0}" = "1" ] || exit 0

input=$(cat)

# Don't re-capture inside a stop-hook continuation.
if [ "$(echo "$input" | jq -r '.stop_hook_active // false')" = "true" ]; then
  exit 0
fi

# Only capture when source files changed this turn.
changed=$(git status --porcelain -- 'apps/**' 'packages/**' 2>/dev/null | grep -E '\.(ts|tsx|js|jsx)' || true)
[ -z "$changed" ] && exit 0

session_id=$(echo "$input" | jq -r '.session_id // ""')
head_sha=$(git rev-parse --short HEAD 2>/dev/null || echo "")
commit_subject=$(git log -1 --pretty=%s 2>/dev/null || echo "")

# Derive a coarse category from a changed path. The distiller refines later.
# The categories/globs below are illustrative — adapt them to this project's own
# domain and directory layout (e.g. swap in your own regulated-data or billing
# category, your own monorepo app/package names).
categorize() {
  case "$1" in
    *rls* | *tenant*) echo "rls-tenant" ;;
    *billing* | *payments*) echo "billing" ;;
    *queue* | *processor* | *worker*) echo "queue" ;;
    *i18n* | *messages*) echo "i18n" ;;
    *audit*) echo "audit-registry" ;;
    packages/db/* | *schema* | *migration*) echo "db-schema" ;;
    apps/web/*) echo "frontend" ;;
    apps/api/*) echo "backend" ;;
    *) echo "uncategorized" ;;
  esac
}

REC="$(git rev-parse --show-toplevel 2>/dev/null)/.claude/scripts/mnemosyne/record-finding.mjs"
[ -f "$REC" ] || exit 0

# One finding per distinct changed file (cap at a sane number to stay cheap).
echo "$changed" | awk '{print $2}' | sort -u | head -20 | while IFS= read -r file; do
  [ -z "$file" ] && continue
  category=$(categorize "$file")
  text=${commit_subject:-"uncommitted change to $file"}
  finding=$(jq -nc \
    --arg source "episodic_fix" \
    --arg finding_text "$text" \
    --arg category "$category" \
    --arg file "$file" \
    --arg action "fixed" \
    --arg commit_sha "$head_sha" \
    --arg session_id "$session_id" \
    '{source:$source, finding_text:$finding_text, category:$category, file:$file,
      accepted:null, action:$action, commit_sha:($commit_sha|select(.!="")),
      session_id:($session_id|select(.!=""))}')
  node "$REC" --json "$finding" >/dev/null 2>&1 || true
done

exit 0
