#!/usr/bin/env bash
# Mnemosyne — nightly distillation (cron entrypoint).
#
# Requires a `rule_findings`/`rule_candidates` table pair in your project's own
# Postgres — see docs/LearnedRulesLoop.md for the schema.
#
# Invokes a headless `claude -p` turn that: runs an optional escape-signal pass
# (e.g. against a runtime error tracker if you have one wired via MCP), queries
# the recent rule_findings window, clusters them, self-checks generalizability,
# checks for conflicts with live candidates, writes surviving clusters as
# rule_candidates, and runs the decay/cap/retention sweep. No ANTHROPIC_API_KEY
# needed — rides your Claude Code subscription.
#
# Hygiene: flock single-instance, timeout, dated log. Must run from inside the
# repo (paths resolve against git rev-parse).
#
# Cron (primary box-cron, staggered off any other nightly data jobs you run):
#   35 3 * * *  cd /path/to/your/project && DATABASE_URL=postgres://... bash \
#     .claude/scripts/mnemosyne/distill.sh >> .claude/.mnemosyne/cron.log 2>&1
set -uo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$REPO_ROOT" || exit 1

if [ -z "${DATABASE_URL:-}" ]; then
  echo "distill: DATABASE_URL env var is required (see docs/LearnedRulesLoop.md)" >&2
  exit 1
fi

ARTIFACT_DIR="$REPO_ROOT/.claude/.mnemosyne"
mkdir -p "$ARTIFACT_DIR"
LOG="$ARTIFACT_DIR/distill-$(date +%F).log"
PROMPT_FILE="$REPO_ROOT/.claude/scripts/mnemosyne/distill-prompt.md"
LOCK="/tmp/mnemosyne-distill.lock"
MODEL="${MNEMOSYNE_DISTILL_MODEL:-claude-sonnet-4-6}"

[ -f "$PROMPT_FILE" ] || { echo "distill: missing $PROMPT_FILE" >&2; exit 1; }

# Single-instance guard — bail (don't queue) if a run is already in flight.
exec 9>"$LOCK"
if ! flock -n 9; then
  echo "distill: another run holds the lock ($LOCK) — skipping" | tee -a "$LOG"
  exit 0
fi

{
  echo "=== mnemosyne distill $(date -Is) (model=$MODEL) ==="
  timeout 900 claude -p "$(cat "$PROMPT_FILE")" \
    --allowedTools "Read,Bash(node *)" \
    --max-turns 40 \
    --model "$MODEL"
  status=$?
  echo "=== distill exit $status $(date -Is) ==="
} >>"$LOG" 2>&1

exit 0
