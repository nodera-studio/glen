#!/usr/bin/env bash
# checkpoint-due.sh — say whether the auto-mode goal's next checkpoint is due (run on every wake).
#
# Usage: bash .claude/scripts/checkpoint-due.sh [<state-file>]
#   default state file: <main checkout>/.claude/auto-mode/state.md (resolved through
#   `git rev-parse --git-common-dir`, so a worktree reads the loop's live state file)
#
# Reads only the state file's `## Checkpoints` section (up to the next `## ` heading):
#   - Cadence: 60m                                    (or `none`)
#   - Last checkpoint: 2026-11-01T08:01:50Z (checkpoint-41)   (or `none`)
# Times are UTC ISO (`date -u +%Y-%m-%dT%H:%M:%SZ`). `Last checkpoint: none` with a cadence is due
# at once, and the next checkpoint is checkpoint-01.
#
# Exit 0 = due or overdue:  checkpoint due: overdue by <N> min (last <ISO>, cadence <C>m); write checkpoint-<NN> now
# Exit 1 = not due:         checkpoint not due: next at <ISO> (in <N> min)
# Exit 3 = the goal has no checkpoints (`Cadence: none`, or no section)
# Exit 2 = error: missing file, a missing or malformed line, a malformed time, no jq. It never
#          guesses: a caller treats exit 2 as "stop and report".
# GLEN_NOW (epoch seconds, read-only env) replaces the clock for the smoke test.
set -u

die() { echo "checkpoint-due: $*" >&2; exit 2; }

[ $# -le 1 ] || die "usage: checkpoint-due.sh [<state-file>]"
command -v jq >/dev/null 2>&1 || die "jq is not installed"
if [ $# -eq 1 ]; then
  STATE="$1"
else
  SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)" || die "cannot resolve the script directory"
  COMMON="$(git -C "$SCRIPT_DIR" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" \
    || die "not inside a git checkout: $SCRIPT_DIR"
  STATE="$(cd "$COMMON/.." && pwd -P)/.claude/auto-mode/state.md"
fi
[ -r "$STATE" ] || die "state file missing or unreadable: $STATE"

# section_value <label>: the text after `- <label>:` in the Checkpoints section (first match).
# Prints `#nosection` when the file has no `## Checkpoints` heading.
section_value() {
  awk -v label="- $1:" '
    /^## / { if (in_sec) exit; if ($0 ~ /^## Checkpoints[[:space:]]*$/) { in_sec = 1; seen = 1 }; next }
    in_sec && index($0, label) == 1 { v = substr($0, length(label) + 1); sub(/^[[:space:]]+/, "", v); print v; exit }
    END { if (!seen) print "#nosection" }
  ' "$STATE"
}

CADENCE_LINE="$(section_value Cadence)"
if [ "$CADENCE_LINE" = "#nosection" ]; then
  echo "checkpoint none: no ## Checkpoints section in $STATE"
  exit 3
fi
[ -n "$CADENCE_LINE" ] || die "no '- Cadence:' line in the Checkpoints section of $STATE"
CADENCE="${CADENCE_LINE%%[[:space:]]*}"
if [ "$CADENCE" = none ]; then
  echo "checkpoint none: the goal has no checkpoints (Cadence: none)"
  exit 3
fi
case "$CADENCE" in
  [1-9]*m) MIN="${CADENCE%m}" ;;
  *) die "malformed cadence (want <minutes>m or none): $CADENCE" ;;
esac
case "$MIN" in *[!0-9]*) die "malformed cadence (want <minutes>m or none): $CADENCE" ;; esac

LAST_LINE="$(section_value 'Last checkpoint')"
[ -n "$LAST_LINE" ] || die "no '- Last checkpoint:' line in the Checkpoints section of $STATE"
LAST="${LAST_LINE%%[[:space:]]*}"

NOW="${GLEN_NOW:-$(date -u +%s)}"
case "$NOW" in '' | *[!0-9]*) die "GLEN_NOW must be epoch seconds: $NOW" ;; esac

if [ "$LAST" = none ]; then
  echo "checkpoint due: overdue by 0 min (last none, cadence ${MIN}m); write checkpoint-01 now"
  exit 0
fi
LAST_S="$(jq -rn --arg t "$LAST" '$t | fromdateiso8601' 2>/dev/null)" \
  || die "malformed Last checkpoint time (want YYYY-MM-DDTHH:MM:SSZ): $LAST"
DUE=$((LAST_S + MIN * 60))

if [ "$NOW" -lt "$DUE" ]; then
  NEXT="$(jq -rn --argjson s "$DUE" '$s | todateiso8601')" || die "cannot format $DUE"
  echo "checkpoint not due: next at $NEXT (in $(((DUE - NOW + 59) / 60)) min)"
  exit 1
fi

N="$(printf '%s\n' "$LAST_LINE" | sed -n 's/.*(checkpoint-\([0-9][0-9]*\)).*/\1/p')"
[ -n "$N" ] || die "Last checkpoint has no '(checkpoint-NN)' to number the next one: $LAST_LINE"
printf 'checkpoint due: overdue by %d min (last %s, cadence %sm); write checkpoint-%02d now\n' \
  $(((NOW - DUE) / 60)) "$LAST" "$MIN" $((10#$N + 1))
exit 0
