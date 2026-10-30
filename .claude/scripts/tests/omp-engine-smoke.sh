#!/usr/bin/env bash
# Offline W9 smoke: invoke the real engine with scratch repos, homes, and CLI stubs.
set -euo pipefail
umask 077

TEST_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$TEST_DIR/../../.." && pwd -P)"
ENGINE="$REPO_ROOT/.claude/scripts/omp-engine.sh"
GUARD="$REPO_ROOT/.claude/hooks/guard-omp-profile.sh"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/glen-omp-smoke.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/tmp" "$WORK/home" "$WORK/bin"
export TMPDIR="$WORK/tmp"
STDOUT="$WORK/stdout" STDERR="$WORK/stderr"
CASES=0 FAILURES=0 CODE=0

assert() {
  local label="$1"
  shift
  CASES=$((CASES + 1))
  if "$@" >/dev/null; then
    printf 'PASS %s\n' "$label"
  else
    printf 'FAIL %s\n' "$label" >&2
    FAILURES=$((FAILURES + 1))
  fi
}
assert_eq() { assert "$1" test "$2" = "$3"; }
assert_exit() { assert_eq "$1" "$CODE" "$2"; }
assert_contains() { assert "$1" grep -Fq -- "$2" "$3"; }
run() {
  if "$@" > "$STDOUT" 2> "$STDERR"; then CODE=0; else CODE="$?"; fi
}
now_ms() { perl -MTime::HiRes=time -e 'printf "%.0f\n", time * 1000'; }
commit_fixture() {
  git -C "$1" -c user.name='Glen smoke' -c user.email='smoke@example.invalid' \
    -c core.hooksPath=/dev/null -c commit.gpgsign=false commit --no-gpg-sign -qm fixture
}

# Any accidental CLI call is a test failure; the real omp is never executed.
cat > "$WORK/bin/glen" <<'EOF'
#!/bin/sh
touch "$SMOKE_UNEXPECTED_CALL"
exit 99
EOF
cp "$WORK/bin/glen" "$WORK/bin/omp"
chmod +x "$WORK/bin/glen" "$WORK/bin/omp"
export PATH="$WORK/bin:$PATH"
export GLEN_BIN="$WORK/bin/glen" SMOKE_UNEXPECTED_CALL="$WORK/unexpected-call"

started="$(now_ms)"
run env GLEN_OMP_ENGINE=off /bin/bash "$ENGINE" probe
elapsed=$(( $(now_ms) - started ))
assert_exit 'probe disabled exit' 3
assert 'probe disabled status' jq -e '.status == "skipped"' "$STDOUT"
assert_contains 'probe disabled reason' GLEN_OMP_ENGINE "$STDOUT"
assert "probe disabled takes under two seconds (${elapsed}ms)" test "$elapsed" -lt 2000

run env HOME="$WORK/home" PI_CONFIG_DIR=.omp GLEN_OMP_ENGINE=on /bin/bash "$ENGINE" probe
assert_exit 'probe missing profile exit' 3
assert_contains 'probe missing profile reason' 'profile is not set up' "$STDOUT"
assert 'probe missing profile creates no run root or jobs' test ! -e "$WORK/tmp/glen-omp-engine"

mkdir -p "$WORK/home/.omp/profiles/glen-harness/agent" "$WORK/home/tmproot"
printf 'tools:\n  approvalMode: always-ask\n' > "$WORK/home/.omp/profiles/glen-harness/agent/config.yml"
run env HOME="$WORK/home" PI_CONFIG_DIR=.omp TMPDIR="$WORK/home/tmproot" GLEN_OMP_ENGINE=on /bin/bash "$ENGINE" probe
assert_exit 'probe run root under home exit' 3
# shellcheck disable=SC2016 # The engine's diagnostic deliberately names HOME literally.
assert_contains 'probe run root under home reason' 'run root under $HOME' "$STDOUT"
assert 'probe fast paths invoke neither glen nor omp' test ! -e "$SMOKE_UNEXPECTED_CALL"

SRC="$WORK/source" DEST="$WORK/snapshot"
mkdir -p "$SRC/.claude" "$SRC/.omp/extensions"
git -C "$SRC" init -q --initial-branch=main
printf 'tracked\n' > "$SRC/tracked file.txt"
printf 'old\n' > "$SRC/modified.txt"
printf 'deleted\n' > "$SRC/deleted.txt"
printf 'ignored.txt\n' > "$SRC/.gitignore"
printf '{}\n' > "$SRC/.claude/settings.json"
printf '// fixture\n' > "$SRC/.omp/extensions/x.ts"
printf '{}\n' > "$SRC/.mcp.json"
printf '# Fixture conventions\n' > "$SRC/AGENTS.md"
git -C "$SRC" add .
commit_fixture "$SRC"
printf 'NEW uncommitted content\n' > "$SRC/modified.txt"
printf 'untracked\n' > "$SRC/untracked.txt"
printf 'ignored\n' > "$SRC/ignored.txt"
rm "$SRC/deleted.txt"
run /bin/bash "$ENGINE" snapshot "$SRC" "$DEST"
assert_exit 'snapshot exit' 0
assert 'snapshot tracked file' test -f "$DEST/tracked file.txt"
assert_eq 'snapshot modified content' "$(cat "$DEST/modified.txt")" 'NEW uncommitted content'
assert 'snapshot untracked file' test -f "$DEST/untracked.txt"
assert 'snapshot excludes ignored file' test ! -e "$DEST/ignored.txt"
assert 'snapshot excludes deleted file' test ! -e "$DEST/deleted.txt"
assert_eq 'snapshot has no git metadata anywhere' "$(find "$DEST" -name .git -print)" ''
assert 'snapshot relocated Claude settings' test -f "$DEST/_agent-config/claude/settings.json"
assert 'snapshot relocated omp extension' test -f "$DEST/_agent-config/omp/extensions/x.ts"
assert 'snapshot relocated MCP config' test -f "$DEST/_agent-config/mcp.json"
for name in .claude .omp .mcp.json; do
  assert "snapshot removed root $name" test ! -e "$DEST/$name"
done
assert 'snapshot keeps root AGENTS.md' test -f "$DEST/AGENTS.md"
assert_eq 'snapshot head' "$(jq -r .head "$STDOUT")" "$(git -C "$SRC" rev-parse HEAD)"
assert 'snapshot hash recorded' jq -e '.snapshot_sha256 | test("^[0-9a-f]{64}$")' "$STDOUT"

# The hook executes with SRC as cwd; each append changes the tracked diff fingerprint.
run env GLEN_OMP_ENGINE_TEST_MUTATE='printf "race\n" >> modified.txt' \
  /bin/bash "$ENGINE" snapshot "$SRC" "$WORK/race-always"
assert_exit 'capture repeated race exit' 3
assert_eq 'capture repeated race status' "$(jq -r .status "$STDOUT")" skipped
assert_eq 'capture repeated race reason' "$(jq -r .reason "$STDOUT")" 'source changed during capture'
run env GLEN_OMP_ENGINE_TEST_MUTATE='if [ ! -e .race-once ]; then printf "once\n" >> modified.txt; : > .race-once; fi' \
  /bin/bash "$ENGINE" snapshot "$SRC" "$WORK/race-once"
assert_exit 'capture one race recaptures successfully' 0
assert_contains 'capture one race retains new source content' once "$WORK/race-once/modified.txt"
assert_eq 'capture one race final status' "$(jq -r .status "$STDOUT")" ok

POLICY_REPO="$WORK/policy-source"
mkdir -p "$POLICY_REPO/.claude/omp"
git -C "$POLICY_REPO" init -q --initial-branch=main
printf '# main policy\ntools:\n  approvalMode: always-ask\n' > "$POLICY_REPO/.claude/omp/glen-harness.yml"
git -C "$POLICY_REPO" add .
commit_fixture "$POLICY_REPO"
printf '# worktree policy\ntools:\n  approvalMode: yolo\n' > "$POLICY_REPO/.claude/omp/glen-harness.yml"
run /bin/bash "$ENGINE" policy "$POLICY_REPO"
assert_exit 'policy exit' 0
assert_eq 'policy source label' "$(sed -n '1p' "$STDOUT")" main
assert_eq 'policy content comes from main' "$(sed '1d' "$STDOUT")" \
  "$(git -C "$POLICY_REPO" show main:.claude/omp/glen-harness.yml)"

# Use a synthetic token. Never print envcheck output or any variable values.
(
  export CLAUDE_CONFIG_DIR=smoke PI_PY=smoke PI_JS=smoke PI_CODING_AGENT_DIR=smoke
  export OMP_CODING_AGENT_DIR=smoke OMP_PROFILE=smoke PI_PROFILE=smoke
  export MNEMOSYNE_TOKEN=smoke-placeholder
  /bin/bash "$ENGINE" envcheck -- env
) > "$WORK/env.out" 2> "$STDERR"
for name in CLAUDE_CONFIG_DIR PI_PY PI_JS PI_CODING_AGENT_DIR OMP_CODING_AGENT_DIR OMP_PROFILE PI_PROFILE; do
  if grep -q "^${name}=" "$WORK/env.out"; then present=yes; else present=no; fi
  assert_eq "dispatch scrubs $name" "$present" no
done
assert 'dispatch retains token variable name' grep -q '^MNEMOSYNE_TOKEN=' "$WORK/env.out"

run_hook() {
  if jq -cn --arg command "$1" '{tool_name:"Bash",tool_input:{command:$command}}' |
    /bin/bash "$GUARD" > "$STDOUT" 2> "$STDERR"; then CODE=0; else CODE="$?"; fi
}
while IFS= read -r command; do
  run_hook "$command"
  assert_exit "guard denies: $command" 2
  assert_contains 'guard deny message on stderr' "This would write the owner's default omp profile" "$STDERR"
  assert 'guard deny has no stdout' test ! -s "$STDOUT"
done <<'EOF'
omp config set theme.dark x
omp login
glen install-extension
cd x && omp plugin install y
omp --profile default config reset theme.dark
omp --profile=default config set a b
omp login && omp --profile glen-harness usage
OMP_PROFILE=glen-harness omp usage; omp logout
echo --dry-run; glen install-extension
omp --profile glen-harness --profile default update
EOF
while IFS= read -r command; do
  run_hook "$command"
  assert_exit "guard allows: $command" 0
  assert 'guard allow has no stdout' test ! -s "$STDOUT"
  assert 'guard allow has no stderr' test ! -s "$STDERR"
done <<'EOF'
omp --profile glen-harness config set a b
glen --profile glen-harness install-extension
glen install-extension --dry-run
omp --version
omp models cursor --json
omp usage accounts --json
omp models update
omp --profile default usage update
PI_CODING_AGENT_DIR=/tmp/x glen install-extension
omp --profile=glen-harness config reset a
OMP_PROFILE=glen-harness omp update
cd x && glen --profile glen-harness install-extension
omp --profile "glen-harness" skills install x
not-omp login
glen install-extension-extra
EOF

printf '%s assertions, %s failures\n' "$CASES" "$FAILURES"
[ "$FAILURES" -eq 0 ]
