#!/usr/bin/env bash
# routing.sh — print who does a kind of work in this harness (one routing source for every prompt).
#
# Usage: bash .claude/scripts/routing.sh <key>
#   e.g. implementation, ui, review, planning, debug, audits, integration,
#        workflow_max_agents, heavy_concurrency, decision, decided
#
# Prints the value on stdout (arrays space-separated), then
# `source=<override|default> decision=<id>` on stderr.
#
# Sources, read from the MAIN checkout (resolved through `git rev-parse --git-common-dir`, so a
# worktree cut before a routing change still reads the current value):
#   1. $MAIN/.claude/auto-mode/routing.json  override (git-ignored, per run), written only on a
#      confirmed owner directive; it may hold only some keys.
#   2. $MAIN/.claude/routing.json            tracked default. If the main checkout has no such
#      file yet (its branch predates it), this script's own checkout's .claude/routing.json.
# GLEN_ROUTING_DIR (read-only env, for the smoke test) replaces $MAIN/.claude, with no fallback.
#
# Uses jq, or node when jq is missing. Exits 2, printing nothing on stdout, on a missing or
# unknown key, unreadable or broken JSON (in either file), or no JSON reader. It never guesses:
# a caller treats any non-zero exit as "stop and report".
set -u

die() { echo "routing: $*" >&2; exit 2; }

[ $# -eq 1 ] && [ -n "$1" ] || die "usage: routing.sh <key>"
KEY="$1"

# json_get <file> <key>: value on stdout; return 3 = key absent or null, 4 = unreadable/broken.
if command -v jq >/dev/null 2>&1; then
  json_get() {
    jq -e 'type == "object"' "$1" >/dev/null 2>&1 || return 4
    [ "$(jq -r --arg k "$2" 'has($k) and .[$k] != null' "$1" 2>/dev/null)" = true ] || return 3
    jq -r --arg k "$2" '.[$k] | if type == "array" then map(tostring) | join(" ")
                                elif type == "object" then tojson else tostring end' "$1" \
      2>/dev/null || return 4
  }
elif command -v node >/dev/null 2>&1; then
  json_get() {
    node -e '
      const fs = require("fs");
      let o;
      try { o = JSON.parse(fs.readFileSync(process.argv[1], "utf8")); } catch (e) { process.exit(4); }
      if (o === null || typeof o !== "object" || Array.isArray(o)) process.exit(4);
      const k = process.argv[2];
      if (!Object.prototype.hasOwnProperty.call(o, k) || o[k] === null) process.exit(3);
      const v = o[k];
      const out = Array.isArray(v) ? v.map(String).join(" ")
        : typeof v === "object" ? JSON.stringify(v) : String(v);
      process.stdout.write(out + "\n");
    ' "$1" "$2"
  }
else
  die "neither jq nor node is installed"
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)" || die "cannot resolve the script directory"
if [ -n "${GLEN_ROUTING_DIR:-}" ]; then
  DIR="$GLEN_ROUTING_DIR"
  DEFAULT="$DIR/routing.json"
else
  COMMON="$(git -C "$SCRIPT_DIR" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" \
    || die "not inside a git checkout: $SCRIPT_DIR"
  MAIN="$(cd "$COMMON/.." && pwd -P)" || die "cannot resolve the main checkout from $COMMON"
  DIR="$MAIN/.claude"
  DEFAULT="$DIR/routing.json"
  [ -e "$DEFAULT" ] || DEFAULT="$(cd "$SCRIPT_DIR/.." && pwd -P)/routing.json"
fi
OVERRIDE="$DIR/auto-mode/routing.json"

[ -r "$DEFAULT" ] || die "default routing file missing or unreadable: $DEFAULT"
DEFAULT_DECISION="$(json_get "$DEFAULT" decision)"
case $? in
  0) ;;
  3) DEFAULT_DECISION=none ;;
  *) die "broken JSON in $DEFAULT" ;;
esac

if [ -e "$OVERRIDE" ]; then
  [ -r "$OVERRIDE" ] || die "override routing file unreadable: $OVERRIDE"
  VALUE="$(json_get "$OVERRIDE" "$KEY")"
  rc=$?
  case $rc in
    0)
      DECISION="$(json_get "$OVERRIDE" decision)" || DECISION=none
      printf '%s\n' "$VALUE"
      echo "source=override decision=$DECISION" >&2
      exit 0
      ;;
    3) ;;
    *) die "broken JSON in $OVERRIDE" ;;
  esac
fi

VALUE="$(json_get "$DEFAULT" "$KEY")"
case $? in
  0) ;;
  3) die "unknown key: $KEY" ;;
  *) die "broken JSON in $DEFAULT" ;;
esac
printf '%s\n' "$VALUE"
echo "source=default decision=$DEFAULT_DECISION" >&2
