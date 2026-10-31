#!/usr/bin/env bash
# Harness batch 10 G4 smoke: steer-bash.sh keeps omp probes off new ~/.omp profiles (s175).
# Feeds PreToolUse(Bash) hook JSON and checks deny versus allow. Needs jq (the hook fails open
# without it, so a missing jq is a failure here, not a pass).
set -euo pipefail

TEST_DIR="$(cd "$(dirname "$0")" && pwd -P)"
HOOK="$(cd "$TEST_DIR/../.." && pwd -P)/hooks/steer-bash.sh"
command -v jq >/dev/null 2>&1 || { echo "FAIL jq is required" >&2; exit 1; }
CASES=0 FAILURES=0

decision() {
  jq -n --arg c "$1" '{tool_name:"Bash",tool_input:{command:$c}}' | /bin/bash "$HOOK" \
    | jq -r '.hookSpecificOutput.permissionDecision // "allow"' 2>/dev/null || true
}

expect() {
  local want="$1" cmd="$2" got
  CASES=$((CASES + 1))
  got=$(decision "$cmd")
  [ -n "$got" ] || got=allow
  if [ "$got" = "$want" ]; then
    printf 'PASS %-5s %s\n' "$want" "$cmd"
  else
    printf 'FAIL want %s got %s: %s\n' "$want" "$got" "$cmd" >&2
    FAILURES=$((FAILURES + 1))
  fi
}

# shellcheck disable=SC2016 # commands are literal hook input, not expanded here
{
  expect deny  'omp --profile glen-harness-probe models'
  expect deny  'PI_PROFILE=x omp models'
  expect deny  'mkdir -p ~/.omp/profiles/foo'
  expect allow 'omp --profile glen-harness models'
  expect allow 'omp --profile=glen-harness models'
  expect allow 'HOME="$T" omp --version'
  expect allow 'ls ~/.omp/profiles'
  # beyond the plan's list
  expect deny  'omp --profile=other models'
  expect deny  'OMP_PROFILE=other omp models'
  expect deny  'HOME=/tmp/x OMP_PROFILE=other omp models'
  expect deny  'export OMP_PROFILE=other && omp models'
  expect deny  'cp -R ~/.omp/profiles/glen-harness ~/.omp/profiles/glen-harness-2'
  expect allow 'OMP_PROFILE=glen-harness omp models'
  expect allow 'mkdir -p ~/.omp/profiles/glen-harness/agent'
  expect allow 'omp --version'
  # wrappers, subshells and sh -c (review LOW-001)
  expect deny  'timeout 30 omp --profile other models'
  expect deny  'timeout -s KILL 30 omp --profile other models'
  expect deny  'env OMP_PROFILE=other omp models'
  expect deny  '(omp --profile other models)'
  expect deny  "sh -c 'omp --profile other models'"
  expect deny  'nice -n 5 /usr/local/bin/omp --profile=other models'
  expect allow 'timeout 30 omp --profile glen-harness models'
  expect allow '(omp --profile glen-harness models)'
  expect allow 'HOME="$T" timeout 30 omp models'
  # only the real home's profiles (review LOW-002)
  expect allow 'mkdir -p "$T/.omp/profiles/fixture"'
  expect allow 'cp -R tests/fixtures/.omp/profiles/x "$T/.omp/profiles/x"'
  expect deny  'mkdir -p "$HOME/.omp/profiles/foo"'
  expect deny  'mkdir -p "${HOME}"/.omp/profiles/foo'
  expect deny  "mkdir -p $HOME/.omp/profiles/foo"
}

printf '%d cases, %d failures\n' "$CASES" "$FAILURES"
[ "$FAILURES" -eq 0 ]
