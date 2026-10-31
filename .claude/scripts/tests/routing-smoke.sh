#!/bin/bash
# routing-smoke.sh — smoke test for .claude/scripts/routing.sh (Bash 3.2, no network).
#
# Usage: /bin/bash .claude/scripts/tests/routing-smoke.sh
# Cases: default only, override present, override for one key only, unknown key (exit 2),
# broken JSON (exit 2). Every case runs twice: with jq, and with node only (a PATH without jq).
# Prints ok/FAIL per case and exits 1 on any FAIL.
set -u

HERE="$(cd "$(dirname "$0")" && pwd -P)"
ROUTING="$HERE/../routing.sh"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/routing-smoke.XXXXXX")" || exit 1
trap 'rm -rf "$TMP"' EXIT
FAILS=0

# A PATH with node and the few tools routing.sh needs, but no jq.
NOJQ="$TMP/bin-nojq"
mkdir -p "$NOJQ"
for t in node dirname; do
  p="$(command -v "$t")" || { echo "FAIL: $t not installed"; exit 1; }
  ln -s "$p" "$NOJQ/$t"
done

DEFAULT_JSON='{"implementation":"claude","ui":"impl-coder-ui","review":["claude","codex"],
"workflow_max_agents":50,"decision":"148655d9","decided":"2026-10-31"}'

# setup <case>: a fresh routing dir with the default file; prints its path.
setup() {
  local d="$TMP/$1"
  mkdir -p "$d/auto-mode"
  printf '%s\n' "$DEFAULT_JSON" >"$d/routing.json"
  printf '%s\n' "$d"
}

# check <name> <engine-path|""> <dir> <key> <want-exit> <want-stdout> <want-stderr-substring>
check() {
  local name="$1" path="$2" dir="$3" key="$4" want_rc="$5" want_out="$6" want_err="$7"
  local out err rc
  if [ -n "$path" ]; then
    out="$(env PATH="$path" GLEN_ROUTING_DIR="$dir" /bin/bash "$ROUTING" "$key" 2>"$TMP/err")"
  else
    out="$(env GLEN_ROUTING_DIR="$dir" /bin/bash "$ROUTING" "$key" 2>"$TMP/err")"
  fi
  rc=$?
  err="$(cat "$TMP/err")"
  case "$err" in *"$want_err"*) ;; *) rc="stderr-mismatch:$rc" ;; esac
  if [ "$rc" = "$want_rc" ] && [ "$out" = "$want_out" ]; then
    echo "ok   $name"
  else
    echo "FAIL $name: exit=$rc (want $want_rc) stdout=[$out] (want [$want_out]) stderr=[$err] (want *$want_err*)"
    FAILS=$((FAILS + 1))
  fi
}

for engine in jq node; do
  if [ "$engine" = jq ]; then
    command -v jq >/dev/null 2>&1 || { echo "FAIL: jq not installed"; FAILS=$((FAILS + 1)); continue; }
    P=""
  else
    P="$NOJQ"
  fi

  # (a) default only.
  d="$(setup "$engine-default")"
  check "$engine: default only, string" "$P" "$d" implementation 0 claude "source=default decision=148655d9"
  check "$engine: default only, array" "$P" "$d" review 0 "claude codex" "source=default decision=148655d9"
  check "$engine: default only, number" "$P" "$d" workflow_max_agents 0 50 "source=default"

  # (b) override present.
  d="$(setup "$engine-override")"
  printf '%s\n' '{"implementation":"codex","review":["codex"],"decision":"abc12345"}' \
    >"$d/auto-mode/routing.json"
  check "$engine: override present" "$P" "$d" implementation 0 codex "source=override decision=abc12345"
  check "$engine: override array" "$P" "$d" review 0 codex "source=override decision=abc12345"

  # (c) override for one key only: other keys still come from the default.
  d="$(setup "$engine-onekey")"
  printf '%s\n' '{"ui":"impl-coder-claude","decision":"def67890"}' >"$d/auto-mode/routing.json"
  check "$engine: one-key override, that key" "$P" "$d" ui 0 impl-coder-claude "source=override decision=def67890"
  check "$engine: one-key override, other key" "$P" "$d" implementation 0 claude "source=default decision=148655d9"

  # (d) unknown key: exit 2, nothing on stdout.
  d="$(setup "$engine-unknown")"
  check "$engine: unknown key" "$P" "$d" nonesuch 2 "" "unknown key: nonesuch"
  printf '%s\n' '{"ui":"impl-coder-ui"}' >"$d/auto-mode/routing.json"
  check "$engine: unknown key with override" "$P" "$d" nonesuch 2 "" "unknown key: nonesuch"

  # (e) broken JSON: exit 2 in the default, and in an override (never falls back).
  d="$(setup "$engine-broken")"
  printf '%s\n' '{"implementation": "claude",' >"$d/routing.json"
  check "$engine: broken default" "$P" "$d" implementation 2 "" "broken JSON"
  d="$(setup "$engine-broken-override")"
  printf '%s\n' '{"implementation": ' >"$d/auto-mode/routing.json"
  check "$engine: broken override" "$P" "$d" implementation 2 "" "broken JSON"
  d="$(setup "$engine-missing")"
  rm "$d/routing.json"
  check "$engine: missing default" "$P" "$d" implementation 2 "" "missing or unreadable"
done

check "no key argument" "" "$TMP" "" 2 "" "usage"

if [ "$FAILS" -gt 0 ]; then
  echo "routing-smoke: $FAILS FAIL"
  exit 1
fi
echo "routing-smoke: all passed"
