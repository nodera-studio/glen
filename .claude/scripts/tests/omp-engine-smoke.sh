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
# Exact AT_IMPORT_REGEX from omp 18.14.0, applied per line as expandTextSegment
# does. Checking code examples too is deliberately stricter than expansion.
has_at_imports() {
  node -e '
    const fs = require("node:fs");
    const regex = /(^|[ \t])@([./~A-Za-z0-9_-](?:\\ |[^\s])*)/g;
    process.exit(fs.readFileSync(process.argv[1], "utf8").split("\n")
      .some(line => { regex.lastIndex = 0; return regex.test(line); }) ? 0 : 1);
  ' "$1"
}
no_at_imports() { ! has_at_imports "$1"; }

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
# Hostile dotenv variants at root, below root, and in a relocated config tree.
mkdir -p "$SRC/nested/.github" "$SRC/.github" "$SRC/.wrangler/state" "$SRC/nested/.wrangler/state"
for path in .env .env.local .env.production .env.production.local nested/.env.development.local .claude/.env; do
  printf 'CLAUDE_CONFIG_DIR=_agent-config/claude\n' > "$SRC/$path"
done
printf 'local database\n' > "$SRC/.wrangler/state/db"
printf 'nested local database\n' > "$SRC/nested/.wrangler/state/db"
printf '@~/.ssh/x\nSee @./local\\ path and\t@../../outside.\nKeep user@example.com readable.\n' >> "$SRC/AGENTS.md"
printf 'See @/outside and @relative\n' > "$SRC/nested/AGENTS.md"
printf 'See @~/private\n' > "$SRC/nested/CLAUDE.md"
printf 'See @../private\n' > "$SRC/.github/copilot-instructions.md"
printf 'See @~/private\n' > "$SRC/nested/.github/copilot-instructions.md"
ln -s AGENTS.md "$SRC/CLAUDE.md"
ln -s nested "$SRC/context-dir"
assert 'context fixture exercises import regex' has_at_imports "$SRC/AGENTS.md"
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
for path in .env .env.local .env.production .env.production.local nested/.env.development.local .claude/.env; do
  assert "snapshot removes dotenv $path" test ! -e "$DEST/$path"
  assert_contains "snapshot preserves dotenv $path" 'CLAUDE_CONFIG_DIR=_agent-config/claude' "$DEST/_agent-config/dotenv/$path"
done
assert_eq 'snapshot has no active dotenv files' "$(find "$DEST" -path "$DEST/_agent-config/dotenv" -prune -o \( -name .env -o -name '.env.*' \) -print)" ''
assert 'snapshot excludes unignored root wrangler state' test ! -e "$DEST/.wrangler"
assert 'snapshot excludes unignored nested wrangler state' test ! -e "$DEST/nested/.wrangler"
assert 'snapshot keeps ordinary untracked content beside state' test -f "$DEST/untracked.txt"
for path in AGENTS.md CLAUDE.md nested/AGENTS.md nested/CLAUDE.md .github/copilot-instructions.md nested/.github/copilot-instructions.md context-dir/AGENTS.md; do
  assert "snapshot neutralizes imports in $path" no_at_imports "$DEST/$path"
done
assert_contains 'snapshot keeps import path readable' '@~/.ssh/x' "$DEST/AGENTS.md"
assert_contains 'snapshot keeps email unchanged' 'user@example.com' "$DEST/AGENTS.md"
assert_eq 'snapshot leaves harmless context unchanged' "$(head -n 1 "$DEST/AGENTS.md")" '# Fixture conventions'
assert 'snapshot accepts internal file symlink' test -L "$DEST/CLAUDE.md"
assert 'snapshot accepts internal directory symlink' test -L "$DEST/context-dir"

# Quarantine is reserved: never merge a copied destination with moved evidence.
mkdir -p "$SRC/_agent-config/dotenv"
printf 'existing evidence\n' > "$SRC/_agent-config/dotenv/keep"
run /bin/bash "$ENGINE" snapshot "$SRC" "$WORK/dotenv-collision"
assert_exit 'dotenv collision fails closed' 4
assert_contains 'dotenv collision reason' 'dotenv relocation collision' "$STDOUT"
assert_contains 'dotenv collision preserves copied evidence' 'existing evidence' "$WORK/dotenv-collision/_agent-config/dotenv/keep"
rm -r "$SRC/_agent-config"

# Exercise tracked and untracked links; no external target is ever read/rewritten.
LINK_SRC="$WORK/link-source"
mkdir -p "$LINK_SRC"
git -C "$LINK_SRC" init -q --initial-branch=main
printf '@~/external-secret\n' > "$WORK/external-context"
ln -s "$WORK/external-context" "$LINK_SRC/AGENTS.md"
git -C "$LINK_SRC" add .
commit_fixture "$LINK_SRC"
run /bin/bash "$ENGINE" snapshot "$LINK_SRC" "$WORK/link-tracked"
assert_exit 'snapshot refuses tracked external context symlink' 4
assert_contains 'snapshot external context symlink reason' 'unsafe symlink' "$STDOUT"
assert_eq 'snapshot never rewrites external symlink target' "$(cat "$WORK/external-context")" '@~/external-secret'
rm "$LINK_SRC/AGENTS.md"
printf 'safe\n' > "$LINK_SRC/plain"
for kind in directory chain dangling cycle; do
  case "$kind" in
    directory) ln -s "$WORK/home" "$LINK_SRC/link" ;;
    chain) ln -s "$WORK/external-context" "$LINK_SRC/first"; ln -s first "$LINK_SRC/link" ;;
    dangling) ln -s "$WORK/nonexistent" "$LINK_SRC/link" ;;
    cycle) ln -s link "$LINK_SRC/link" ;;
  esac
  run /bin/bash "$ENGINE" snapshot "$LINK_SRC" "$WORK/link-$kind"
  assert_exit "snapshot refuses untracked $kind symlink" 4
  assert_contains "snapshot $kind symlink reason" 'unsafe symlink' "$STDOUT"
  rm "$LINK_SRC/link"
  [ "$kind" != chain ] || rm "$LINK_SRC/first"
done
mkdir -p "$LINK_SRC/.claude"
ln -s ../plain "$LINK_SRC/.claude/link"
run /bin/bash "$ENGINE" snapshot "$LINK_SRC" "$WORK/link-relocation"
assert_exit 'snapshot revalidates relative links after relocation' 4
assert_contains 'snapshot relocation symlink reason' 'unsafe symlink' "$STDOUT"
rm "$LINK_SRC/.claude/link"
ln -s plain "$LINK_SRC/AGENTS.md"
run /bin/bash "$ENGINE" snapshot "$LINK_SRC" "$WORK/link-safe"
assert_exit 'snapshot accepts tracked internal context symlink' 0
assert_eq 'snapshot internal context symlink content' "$(cat "$WORK/link-safe/AGENTS.md")" safe

# Use real du/copy accounting with allocated, nonzero data (BSD tar can retain
# sparse holes). Move the same fixture from ignored to tracked to test both sides.
SIZE_SRC="$WORK/size-source"
mkdir -p "$SIZE_SRC/ignored-build"
git -C "$SIZE_SRC" init -q --initial-branch=main
printf 'ignored-build/\n' > "$SIZE_SRC/.gitignore"
printf 'small\n' > "$SIZE_SRC/small"
git -C "$SIZE_SRC" add .
commit_fixture "$SIZE_SRC"
dd if=/dev/urandom of="$SIZE_SRC/ignored-build/large" bs=1048576 count=501 2>/dev/null
assert 'size fixture ignored output actually exceeds limit' test "$(du -sk "$SIZE_SRC" | awk '{print $1}')" -gt 512000
run /bin/bash "$ENGINE" snapshot "$SIZE_SRC" "$WORK/size-small"
assert_exit 'snapshot ignores oversized ignored build output' 0
assert 'snapshot small tree copied' test -f "$WORK/size-small/small"
assert 'snapshot oversized ignored build output absent' test ! -e "$WORK/size-small/ignored-build"
mv "$SIZE_SRC/ignored-build/large" "$SIZE_SRC/large-tracked"
git -C "$SIZE_SRC" add large-tracked
run /bin/bash "$ENGINE" snapshot "$SIZE_SRC" "$WORK/size-large"
assert_exit 'snapshot oversized selected tree exit' 3
assert_eq 'snapshot oversized selected tree status' "$(jq -r .status "$STDOUT")" skipped
assert_eq 'snapshot oversized selected tree reason unchanged' "$(jq -r .reason "$STDOUT")" 'snapshot over 500 MB'
rm "$SIZE_SRC/large-tracked" "$WORK/size-large/large-tracked"

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
# shellcheck disable=SC2016 # Expanded by the snapshot subprocess, not this shell.
run env SMOKE_MUTATION_MARKER="$WORK/direct-mutation" GLEN_OMP_ENGINE_TEST_MUTATE=': > "$SMOKE_MUTATION_MARKER"' \
  /bin/bash "$ENGINE" snapshot "$SRC" "$WORK/direct-mutation-snapshot"
assert_exit 'standalone snapshot explicit mutation hook exit' 0
assert 'standalone snapshot honors explicit mutation hook' test -f "$WORK/direct-mutation"

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

# Successful production paths use offline stubs, never the owner's omp/profile.
export SMOKE_OMP_CALLS="$WORK/omp-calls" SMOKE_DISPATCH_CALLS="$WORK/dispatch-calls"
export SMOKE_SESSION="$WORK/session.jsonl"
printf '%s\n' '{"type":"message","message":{"role":"assistant","model":"fixture","provider":"cursor","usage":{"input":1,"output":1}}}' > "$SMOKE_SESSION"
cat > "$WORK/bin/omp" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$SMOKE_OMP_CALLS"
case "$*" in
  '--profile glen-harness models cursor --json') printf '%s\n' '{"models":[{"id":"fixture"}]}' ;;
  '--profile glen-harness config get modelRoles.default --json') printf '%s\n' '{"value":"cursor/fixture"}' ;;
  *) exit 99 ;;
esac
EOF
cat > "$WORK/bin/glen" <<'EOF'
#!/bin/bash
set -eu
if [ "${1:-}" = --profile ]; then shift 2; fi
command="$1"
shift
# The send AND wait environment must retain the dotenv backstop.
[ "${CLAUDE_CONFIG_DIR+x}" != x ] || exit 99
printf '%s scrubbed\n' "$command" >> "$SMOKE_DISPATCH_CALLS"
case "$command" in
  send)
    while [ "$#" -gt 0 ]; do
      if [ "$1" = --cwd ]; then cwd="$2"; break; fi
      shift
    done
    [ -n "${cwd:-}" ] && [ ! -e "$cwd/.env" ] || exit 99
    cat >/dev/null
    answer=''
    code=10
    ;;
  wait)
    if [ "$SMOKE_ENGINE_MODE" = review ]; then answer=$'## omp review\nAPPROVE\nMnemosyne: unavailable'; else answer=$'## Root Cause\nFixture'; fi
    code=0
    ;;
  *) exit 99 ;;
esac
jq -cn --arg answer "$answer" --arg session "$SMOKE_SESSION" \
  '{data:{assistant_text:$answer,job:{handle:"j-fixture"},session:{handle:"s-fixture",path:$session},replies:[]}}'
exit "$code"
EOF
printf 'Debug fixture\n' > "$WORK/brief.md"
run env HOME="$WORK/home" PI_CONFIG_DIR=.omp GLEN_OMP_ENGINE=on /bin/bash "$ENGINE" probe
assert_exit 'probe safe private run root exit' 0
assert_eq 'probe safe private run root status' "$(jq -r .status "$STDOUT")" ready
assert_eq 'probe private run root permissions' "$(perl -e 'printf "%04o\n", (stat shift)[2] & 07777' "$WORK/tmp/glen-omp-engine")" 0700

# A leaked orchestrator hook must not execute inside the production snapshot,
# while both review/debug still reach a send AND a wait with scrubbed env.
for mode in review debug; do
  if [ "$mode" = review ]; then mode_args=(--uncommitted); else mode_args=(--brief "$WORK/brief.md"); fi
  for hook in clean inherited; do
    (
      if [ "$hook" = inherited ]; then
        # shellcheck disable=SC2016 # The snapshot hook must expand its own env.
        export GLEN_OMP_ENGINE_TEST_MUTATE=': > "$SMOKE_MUTATION_MARKER"; printf "leaked hook\n" >> modified.txt'
      else
        unset GLEN_OMP_ENGINE_TEST_MUTATE
      fi
      run env HOME="$WORK/home" PI_CONFIG_DIR=.omp GLEN_OMP_ENGINE=on CLAUDE_CONFIG_DIR=hostile \
        SMOKE_ENGINE_MODE="$mode" SMOKE_MUTATION_MARKER="$WORK/production-mutation-$mode" \
        /bin/bash "$ENGINE" "$mode" --repo "$SRC" "${mode_args[@]}" --out "$WORK/$mode-$hook.md"
      printf '%s\n' "$CODE" > "$WORK/production-code"
    )
    CODE="$(cat "$WORK/production-code")"
    assert_exit "$mode $hook mutation environment production exit" 0
    assert_eq "$mode $hook mutation environment production status" "$(jq -r .status "$STDOUT")" ok
    assert "$mode $hook mutation environment never executes hook" test ! -e "$WORK/production-mutation-$mode"
  done
done
assert_eq 'production send environment scrubbed on every call' "$(grep -c '^send scrubbed$' "$SMOKE_DISPATCH_CALLS")" 4
assert_eq 'production wait environment scrubbed on every call' "$(grep -c '^wait scrubbed$' "$SMOKE_DISPATCH_CALLS")" 4
if grep -q 'leaked hook' "$SRC/modified.txt"; then leaked=yes; else leaked=no; fi
assert_eq 'production mutation leaves source unchanged' "$leaked" no

# Check unsafe bits on the canonical directory, including a symlinked root.
# A different-owner fixture requires chown privileges unavailable in normal CI.
omp_calls_before="$(wc -l < "$SMOKE_OMP_CALLS")"
for permissions in 0770 0702; do
  chmod "$permissions" "$WORK/tmp/glen-omp-engine"
  for mode in probe review debug; do
    case "$mode" in
      probe) mode_args=(probe) ;;
      review) mode_args=(review --repo "$SRC" --uncommitted --out "$WORK/unsafe-review.md") ;;
      debug) mode_args=(debug --repo "$SRC" --brief "$WORK/brief.md" --out "$WORK/unsafe-debug.md") ;;
    esac
    run env HOME="$WORK/home" PI_CONFIG_DIR=.omp GLEN_OMP_ENGINE=on /bin/bash "$ENGINE" "${mode_args[@]}"
    assert_exit "$mode refuses unsafe root permissions $permissions" 3
    assert_eq "$mode unsafe root status $permissions" "$(jq -r .status "$STDOUT")" skipped
    assert_contains "$mode unsafe root reason $permissions" 'unsafe run root:' "$STDOUT"
    assert_contains "$mode unsafe root names permission problem $permissions" 'group- or world-writable' "$STDOUT"
  done
done
chmod 0700 "$WORK/tmp/glen-omp-engine"
mkdir -p "$WORK/symlink-tmp" "$WORK/symlink-target/glen-omp-engine"
chmod 0770 "$WORK/symlink-target/glen-omp-engine"
ln -s "$WORK/symlink-target/glen-omp-engine" "$WORK/symlink-tmp/glen-omp-engine"
run env HOME="$WORK/home" PI_CONFIG_DIR=.omp TMPDIR="$WORK/symlink-tmp" GLEN_OMP_ENGINE=on /bin/bash "$ENGINE" probe
assert_exit 'probe refuses unsafe symlink target run root' 3
assert_contains 'probe unsafe symlink target reason names root' "$(cd "$WORK/symlink-target/glen-omp-engine" && pwd -P)" "$STDOUT"
assert_eq 'unsafe run roots invoke no omp probes or jobs' "$(wc -l < "$SMOKE_OMP_CALLS")" "$omp_calls_before"

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
