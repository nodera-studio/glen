#!/bin/bash
# checkpoint-due-smoke.sh — smoke test for .claude/scripts/checkpoint-due.sh (Bash 3.2, no network).
#
# Usage: /bin/bash .claude/scripts/tests/checkpoint-due-smoke.sh
# Cases (temp state files, GLEN_NOW fixed): overdue, not due, Cadence: none, no section,
# Last checkpoint: none, malformed time, a 90m cadence, a later `## ` section not read.
# Prints ok/FAIL per case and exits 1 on any FAIL. Needs jq.
set -u

HERE="$(cd "$(dirname "$0")" && pwd -P)"
DUE="$HERE/../checkpoint-due.sh"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/checkpoint-due-smoke.XXXXXX")" || exit 1
trap 'rm -rf "$TMP"' EXIT
FAILS=0
command -v jq >/dev/null 2>&1 || { echo "FAIL: jq not installed"; exit 1; }

epoch() { jq -rn --arg t "$1" '$t | fromdateiso8601'; }

# state <name> <body>: a state file with a Goal section before the body; prints its path.
state() {
  printf '# Auto-mode state\n\n## Goal\n\n- Cadence: none\n\n%s\n' "$2" >"$TMP/$1.md"
  printf '%s\n' "$TMP/$1.md"
}

# check <name> <file> <now-ISO> <want-exit> <want-stdout-substring>...
check() {
  local name="$1" file="$2" now="$3" want_rc="$4" out rc s miss=""
  shift 4
  out="$(env GLEN_NOW="$(epoch "$now")" /bin/bash "$DUE" "$file" 2>&1)"
  rc=$?
  for s in "$@"; do
    case "$out" in *"$s"*) ;; *) miss="$miss [$s]" ;; esac
  done
  if [ "$rc" = "$want_rc" ] && [ -z "$miss" ]; then
    echo "ok   $name"
  else
    echo "FAIL $name: exit=$rc (want $want_rc) output=[$out] missing:$miss"
    FAILS=$((FAILS + 1))
  fi
}

SECTION='## Checkpoints

- Cadence: 60m
- Last checkpoint: 2026-11-01T08:01:50Z (checkpoint-41)
- Path: challenge/checkpoints/checkpoint-NN.md
- Schedule: none

## Processes started

| pid | started (ps lstart) | command | why | stopped |
| --- | --- | --- | --- | --- |'

f="$(state normal "$SECTION")"
check "overdue" "$f" 2026-11-01T09:30:00Z 0 "checkpoint due: overdue by 28 min" \
  "last 2026-11-01T08:01:50Z, cadence 60m" "write checkpoint-42 now"
check "not due" "$f" 2026-11-01T08:30:00Z 1 "checkpoint not due: next at 2026-11-01T09:01:50Z (in 32 min)"
check "due exactly at the cadence" "$f" 2026-11-01T09:01:50Z 0 "overdue by 0 min" "checkpoint-42"

f="$(state none-cadence '## Checkpoints

- Cadence: none
- Last checkpoint: none')"
check "Cadence: none" "$f" 2026-11-01T09:30:00Z 3 "Cadence: none"

f="$(state no-section '## Status

- Running.')"
check "no section" "$f" 2026-11-01T09:30:00Z 3 "no ## Checkpoints section"

f="$(state first '## Checkpoints

- Cadence: 60m            (the goal asks for hourly checkpoints)
- Last checkpoint: none')"
check "Last checkpoint: none" "$f" 2026-11-01T09:30:00Z 0 "checkpoint due" "write checkpoint-01 now"

f="$(state malformed '## Checkpoints

- Cadence: 60m
- Last checkpoint: 2026-11-01 08:01 (checkpoint-41)')"
check "malformed time" "$f" 2026-11-01T09:30:00Z 2 "malformed Last checkpoint time"

f="$(state bad-cadence '## Checkpoints

- Cadence: hourly
- Last checkpoint: 2026-11-01T08:01:50Z (checkpoint-41)')"
check "malformed cadence" "$f" 2026-11-01T09:30:00Z 2 "malformed cadence"

f="$(state ninety '## Checkpoints

- Cadence: 90m
- Last checkpoint: 2026-11-01T08:01:50Z (checkpoint-09)')"
check "90m not due at 09:30" "$f" 2026-11-01T09:30:00Z 1 "next at 2026-11-01T09:31:50Z (in 2 min)"
check "90m due at 10:00" "$f" 2026-11-01T10:00:00Z 0 "overdue by 28 min" "cadence 90m" "checkpoint-10"

# Lines in a later `## ` section are never read: here only it has the Cadence line.
f="$(state later '## Checkpoints

- Last checkpoint: 2026-11-01T08:01:50Z (checkpoint-41)

## Notes

- Cadence: 60m')"
check "later section not read" "$f" 2026-11-01T09:30:00Z 2 "no '- Cadence:' line"
f="$(state later2 "$SECTION

## Notes

- Cadence: none
- Last checkpoint: none")"
check "later section does not override" "$f" 2026-11-01T09:30:00Z 0 "checkpoint-42"

check "missing state file" "$TMP/absent.md" 2026-11-01T09:30:00Z 2 "missing or unreadable"

if [ "$FAILS" -gt 0 ]; then
  echo "checkpoint-due-smoke: $FAILS FAIL"
  exit 1
fi
echo "checkpoint-due-smoke: all passed"
