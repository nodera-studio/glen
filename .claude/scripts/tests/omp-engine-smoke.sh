#!/usr/bin/env bash
# W9 smoke: scratch repos/homes and CLI stubs, plus gated macOS sandbox checks.
set -euo pipefail
umask 077

TEST_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$TEST_DIR/../../.." && pwd -P)"
ENGINE="$REPO_ROOT/.claude/scripts/omp-engine.sh"
SANDBOX="$REPO_ROOT/.claude/omp/omp-sandboxed.sh"
LINUX_SANDBOX="$REPO_ROOT/.claude/omp/omp-sandboxed-linux.sh"
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
# Glen pipes stdout (src/omp/bin.rs:89-91); RPC pipes stdin/stdout (omp-rpc sdk/rust/omp-rpc/src/client.rs:756-758).
run_sandbox() {
  # Keep the launcher's status, including expected denials, under errexit/pipefail.
  if "$@" 2> "$STDERR" | cat > "$STDOUT"; then
    CODE="${PIPESTATUS[0]}"
  else
    CODE="${PIPESTATUS[0]}"
  fi
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

# Exercise the launcher's own generated policy, never a handwritten test policy.
# Real omp startup runs only when installed, before PATH gains CLI stubs.
if [ "$(uname -s)" = Darwin ] && command -v sandbox-exec >/dev/null 2>&1; then
  run_sandbox sandbox-exec -p '(version 1)(allow default)' /usr/bin/true
  sandbox_runtime=0
  if [ "$CODE" -eq 0 ]; then
    sandbox_runtime=1
  else
    printf 'NOTE sandbox runtime checks skipped: allow-all sandbox-exec self-test failed (exit %s): %s\n' "$CODE" "$(cat "$STDERR")"
  fi
  real_omp="$(command -v omp || true)"
  sandbox_base="$WORK/sandbox"
  mkdir -p "$sandbox_base/home/.ssh" "$sandbox_base/home/.omp/agent" \
    "$sandbox_base/home/.omp/profiles/glen-harness/agent" "$sandbox_base/run" "$sandbox_base/bin"
  sandbox_base="$(cd "$sandbox_base" && pwd -P)"
  # Exercise aliases only in launcher-owned inputs, including /var under TMPDIR.
  # The executable has a relative symlink chain as well as aliased ancestors.
  ln -s "$sandbox_base" "$WORK/sandbox-alias"
  sandbox_alias="$WORK/sandbox-alias"
  case "$sandbox_base" in /private/var/*) sandbox_alias="${sandbox_base#/private}" ;; esac
  sandbox_home="$sandbox_alias/home" sandbox_run="$sandbox_alias/run"
  ln -s /bin/cat "$sandbox_base/bin/helper"
  ln -s helper "$sandbox_base/bin/omp"
  printf 'allowed fixture\n' > "$sandbox_base/run/readable"
  printf 'ssh fixture\n' > "$sandbox_base/home/.ssh/known_hosts"
  printf 'canary fixture\n' > "$sandbox_base/home/glen-canary.txt"
  printf 'default config fixture\n' > "$sandbox_base/home/.omp/agent/config.yml"
  printf 'parent context fixture\n' > "$sandbox_base/AGENTS.md"

  sandbox_call() (
    local binary="$1"
    shift
    # Like omp-engine.sh, give the child physical cwd and file arguments.
    cd "$sandbox_base/run" || exit 1
    env -u CLAUDE_CONFIG_DIR -u PI_PY -u PI_JS -u PI_CONFIG_FILES \
      -u PI_CODING_AGENT_DIR -u OMP_CODING_AGENT_DIR -u OMP_PROFILE -u PI_PROFILE \
      -u XDG_STATE_HOME -u XDG_CONFIG_HOME -u XDG_CACHE_HOME -u XDG_DATA_HOME \
      LC_ALL=C HOME="$sandbox_home" PI_CONFIG_DIR=.omp OMP_REAL_BIN="$binary" \
      OMP_SANDBOX_RUN_ROOT="$sandbox_run" /bin/bash "$SANDBOX" "$@"
  )
  run_sandbox sandbox_call "$sandbox_alias/bin/omp" "$sandbox_base/run/readable"
  if [ "$sandbox_runtime" -eq 1 ]; then
    assert_exit 'sandbox helper reads allowed run root' 0
    assert_eq 'sandbox helper read content' "$(cat "$STDOUT")" 'allowed fixture'
  fi
  profiles=("$sandbox_base/run"/omp-sandbox.*)
  generated_profile="${profiles[0]}"
  assert_contains 'sandbox imports dyld bootstrap support' '(import "dyld-support.sb")' "$generated_profile"
  assert_contains 'sandbox canonicalizes run root' "(subpath \"$sandbox_base/run\")" "$generated_profile"
  assert_contains 'sandbox canonicalizes HOME' "(subpath \"$sandbox_base/home/.omp/profiles/glen-harness\")" "$generated_profile"
  assert_contains 'sandbox resolves executable symlink chain' "(literal \"$(cd /bin && pwd -P)/cat\")" "$generated_profile"
  assert 'sandbox profile contains no unresolved fixture alias' test "$(grep -Fc sandbox-alias "$generated_profile" || true)" -eq 0
  # An opening quote followed by /var/ must never survive physical resolution.
  assert 'sandbox profile contains no logical /var literal' test "$(grep -Fc '\"/var/' "$generated_profile" || true)" -eq 0

  if [ "$sandbox_runtime" -eq 1 ]; then
    # shellcheck disable=SC2016 # The sandboxed child must expand its own HOME.
    run_sandbox sandbox_call /bin/sh -c 'printf "%s\n" "$HOME"'
    assert_eq 'sandbox child inherits canonical HOME' "$(cat "$STDOUT")" "$sandbox_base/home"
    for denied in home/.ssh/known_hosts home/glen-canary.txt home/.omp/agent/config.yml AGENTS.md; do
      run_sandbox sandbox_call /bin/cat "$sandbox_base/$denied"
      assert "sandbox denies outside read $denied" test "$CODE" -ne 0
      # Match cat's file-specific denial, not sandbox_apply failing before exec.
      assert_contains "sandbox outside read is EPERM $denied" "cat: $sandbox_base/$denied: Operation not permitted" "$STDERR"
    done
    for writable in "$sandbox_base/run/writable" "$sandbox_base/home/.omp/profiles/glen-harness/agent/writable"; do
      run_sandbox sandbox_call /usr/bin/tee "$writable" <<< 'sandbox write'
      assert_exit 'sandbox allows write in permitted directory' 0
      run_sandbox sandbox_call /bin/cat "$writable"
      assert_exit 'sandbox allows read-back in permitted directory' 0
      assert_eq 'sandbox write read-back content' "$(cat "$STDOUT")" 'sandbox write'
    done
  fi

  mkdir "$sandbox_base/no-sandbox-bin"
  ln -s "$(command -v uname)" "$sandbox_base/no-sandbox-bin/uname"
  run_sandbox env PATH="$sandbox_base/no-sandbox-bin" HOME="$sandbox_home" \
    OMP_REAL_BIN=/bin/cat OMP_SANDBOX_RUN_ROOT="$sandbox_run" /bin/bash "$SANDBOX" "$sandbox_base/run/readable"
  assert 'sandbox missing executable fails nonzero' test "$CODE" -ne 0
  assert_contains 'sandbox missing executable explains refusal' 'sandbox-exec not on PATH; refusing to run omp unsandboxed' "$STDERR"
  assert 'sandbox missing executable never runs helper' test ! -s "$STDOUT"

  if [ "$sandbox_runtime" -eq 1 ] && [ -n "$real_omp" ]; then
    # Always select glen-harness, including --version. Only scratch HOME is used.
    run_sandbox sandbox_call "$real_omp" --profile glen-harness --version
    assert_exit 'sandbox real omp version completes' 0
    run_sandbox sandbox_call "$real_omp" --profile glen-harness models --json
    assert_exit 'sandbox real omp models completes without login' 0
  elif [ -z "$real_omp" ]; then
    printf 'NOTE sandbox real omp startup skipped: omp not on PATH\n'
  fi
else
  printf 'NOTE macOS sandbox checks skipped: Darwin with sandbox-exec required\n'
fi

# Outside the gated startup checks, any accidental CLI call is a test failure.
cat > "$WORK/bin/glen" <<'EOF'
#!/bin/sh
touch "$SMOKE_UNEXPECTED_CALL"
exit 99
EOF
cp "$WORK/bin/glen" "$WORK/bin/omp"
chmod +x "$WORK/bin/glen" "$WORK/bin/omp"
export PATH="$WORK/bin:$PATH"
export GLEN_BIN="$WORK/bin/glen" SMOKE_UNEXPECTED_CALL="$WORK/unexpected-call"

# Linux coverage is limited to local probe gates; no live omp or real bwrap run.
if [ "$(uname -s)" != Darwin ]; then
  linux_probe_bin="$WORK/linux-probe-bin"
  mkdir -p "$linux_probe_bin" "$WORK/linux-probe-home"
  # An allowlisted PATH guarantees bwrap is absent even when CI installed it.
  for tool in date uname git dirname jq omp; do
    ln -s "$(command -v "$tool")" "$linux_probe_bin/$tool"
  done
  run env HOME="$WORK/linux-probe-home" PI_CONFIG_DIR=.omp GLEN_OMP_ENGINE=on \
    PATH="$linux_probe_bin" /bin/bash "$ENGINE" probe
  assert_exit 'Linux probe missing bwrap skips' 3
  assert 'Linux probe missing bwrap status' jq -e '.status == "skipped"' "$STDOUT"
  assert_contains 'Linux probe missing bwrap reason' 'bwrap not on PATH; the omp sandbox requires it on Linux' "$STDOUT"

  # Keep the fake tool for the rest of the offline engine fixtures as well.
  # Any attempt to execute it is unexpected: only command -v should inspect it.
  cp "$WORK/bin/glen" "$WORK/bin/bwrap"
  chmod +x "$WORK/bin/bwrap"
  run env HOME="$WORK/linux-probe-home" PI_CONFIG_DIR=.omp GLEN_OMP_ENGINE=on \
    PATH="$WORK/bin:$linux_probe_bin" /bin/bash "$ENGINE" probe
  assert_exit 'Linux probe fake bwrap proceeds to next skip' 3
  assert_contains 'Linux probe fake bwrap reaches profile check' 'profile is not set up' "$STDOUT"
  assert 'Linux probe tool checks invoke neither omp nor bwrap' test ! -e "$SMOKE_UNEXPECTED_CALL"
else
  printf 'NOTE Linux bwrap probe checks skipped: non-Darwin required\n'
fi

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
mkdir -p "$SRC/.claude" "$SRC/.omp/extensions" "$SRC/.agent/rules"
git -C "$SRC" init -q --initial-branch=main
printf 'tracked\n' > "$SRC/tracked file.txt"
printf 'old\n' > "$SRC/modified.txt"
printf 'deleted\n' > "$SRC/deleted.txt"
printf 'ignored.txt\n' > "$SRC/.gitignore"
printf '{}\n' > "$SRC/.claude/settings.json"
printf '// fixture\n' > "$SRC/.omp/extensions/x.ts"
printf '{}\n' > "$SRC/.mcp.json"
printf 'agent rule\n' > "$SRC/.agent/rules/fixture.md"
printf 'cline rule\n' > "$SRC/.clinerules"
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
assert 'snapshot relocated agent rules' test -f "$DEST/_agent-config/agent/rules/fixture.md"
assert_contains 'snapshot relocated clinerules content' 'cline rule' "$DEST/_agent-config/clinerules"
for name in .claude .omp .mcp.json .agent .clinerules; do
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
  export GLEN_OMP_BIN=hostile-inherited OMP_SANDBOX_RUN_ROOT=hostile-inherited
  export MNEMOSYNE_TOKEN=smoke-placeholder
  /bin/bash "$ENGINE" envcheck -- env
) > "$WORK/env.out" 2> "$STDERR"
for name in CLAUDE_CONFIG_DIR PI_PY PI_JS PI_CODING_AGENT_DIR OMP_CODING_AGENT_DIR OMP_PROFILE PI_PROFILE; do
  if grep -q "^${name}=" "$WORK/env.out"; then present=yes; else present=no; fi
  assert_eq "dispatch scrubs $name" "$present" no
done
assert 'dispatch retains token variable name' grep -q '^MNEMOSYNE_TOKEN=' "$WORK/env.out"
if [ "$(uname -s)" = Darwin ] && command -v sandbox-exec >/dev/null 2>&1; then
  assert_contains 'dispatch selects executable sandbox launcher' "GLEN_OMP_BIN=$SANDBOX" "$WORK/env.out"
  assert 'sandbox launcher is a regular executable file' test -f "$SANDBOX" -a -x "$SANDBOX"
elif [ "$(uname -s)" != Darwin ] && command -v bwrap >/dev/null 2>&1; then
  assert_contains 'dispatch selects executable Linux sandbox launcher' "GLEN_OMP_BIN=$LINUX_SANDBOX" "$WORK/env.out"
  assert 'Linux sandbox launcher is a regular executable file' test -f "$LINUX_SANDBOX" -a -x "$LINUX_SANDBOX"
else
  assert 'dispatch without sandbox tool leaves GLEN_OMP_BIN unset' test "$(grep -c '^GLEN_OMP_BIN=' "$WORK/env.out" || true)" -eq 0
fi

# Successful production paths use offline stubs, never the owner's omp/profile.
export SMOKE_OMP_CALLS="$WORK/omp-calls" SMOKE_DISPATCH_CALLS="$WORK/dispatch-calls"
export SMOKE_SESSION="$WORK/session.jsonl"
printf '%s\n' '{"type":"message","message":{"role":"assistant","model":"fixture","provider":"cursor","usage":{"input":1,"output":1}}}' > "$SMOKE_SESSION"
cat > "$WORK/bin/omp" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$SMOKE_OMP_CALLS"
case "$*" in
  '--profile glen-harness models cursor --json') printf '%s\n' '{"models":[{"id":"fixture"}]}' ;;
  '--profile glen-harness config get modelRoles --json')
    [ "${SMOKE_REJECT_MODEL_ROLES:-0}" = 0 ] || exit 99
    printf '%s\n' '{"value":{"default":"cursor/fixture","task":"cursor/other"}}'
    ;;
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
if [ "$(uname -s)" = Darwin ] && command -v sandbox-exec >/dev/null 2>&1; then
  [ "${GLEN_OMP_BIN:-}" = "$SMOKE_SANDBOX" ] || exit 99
  [ -d "${OMP_SANDBOX_RUN_ROOT:-}/tree" ] || exit 99
elif [ "$(uname -s)" != Darwin ] && command -v bwrap >/dev/null 2>&1; then
  [ "${GLEN_OMP_BIN:-}" = "$SMOKE_LINUX_SANDBOX" ] || exit 99
  [ -d "${OMP_SANDBOX_RUN_ROOT:-}/tree" ] || exit 99
fi
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
export SMOKE_SANDBOX="$SANDBOX"
export SMOKE_LINUX_SANDBOX="$LINUX_SANDBOX"
printf 'Debug fixture\n' > "$WORK/brief.md"
run env HOME="$WORK/home" PI_CONFIG_DIR=.omp GLEN_OMP_ENGINE=on /bin/bash "$ENGINE" probe
assert_exit 'probe safe private run root exit' 0
assert_eq 'probe safe private run root status' "$(jq -r .status "$STDOUT")" ready
assert_eq 'probe private run root permissions' "$(perl -e 'printf "%04o\n", (stat shift)[2] & 07777' "$WORK/tmp/glen-omp-engine")" 0700
assert_contains 'probe queries registered modelRoles record key' 'config get modelRoles --json' "$SMOKE_OMP_CALLS"
assert_eq 'probe never queries invalid modelRoles.default key' "$(grep -c 'config get modelRoles.default' "$SMOKE_OMP_CALLS" || true)" 0
run env HOME="$WORK/home" PI_CONFIG_DIR=.omp GLEN_OMP_ENGINE=on SMOKE_REJECT_MODEL_ROLES=1 /bin/bash "$ENGINE" probe
assert_exit 'probe rejected modelRoles query skips' 3
assert_contains 'probe rejected modelRoles query reason' 'config check failed' "$STDOUT"

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

# Full dispatch integration. Clock/sleep wrappers exercise the real retry/deadline
# branches without sleeping a minute or adding a production-only test hook.
DISPATCH_SRC="$WORK/dispatch-source" DISPATCH_BIN="$WORK/dispatch-bin"
mkdir -p "$DISPATCH_SRC" "$DISPATCH_BIN" "$WORK/dispatch"
git -C "$DISPATCH_SRC" init -q --initial-branch=main
printf 'fixture\n' > "$DISPATCH_SRC/tracked"
git -C "$DISPATCH_SRC" add .
commit_fixture "$DISPATCH_SRC"
FAKE_REAL_DATE="$(command -v date)" FAKE_REAL_DU="$(command -v du)"
export FAKE_REAL_DATE FAKE_REAL_DU
cat > "$DISPATCH_BIN/date" <<'EOF'
#!/bin/bash
set -eu
if [ "$*" = '+%s' ]; then cat "$FAKE_GLEN_STATE/clock"; else exec "$FAKE_REAL_DATE" "$@"; fi
EOF
cat > "$DISPATCH_BIN/sleep" <<'EOF'
#!/bin/bash
set -eu
[ "$*" = 60 ] || exit 99
printf '60\n' >> "$FAKE_GLEN_STATE/sleeps"
printf '%s\n' "$(( $(cat "$FAKE_GLEN_STATE/clock") + 60 ))" > "$FAKE_GLEN_STATE/clock"
EOF
cat > "$DISPATCH_BIN/du" <<'EOF'
#!/bin/bash
set -eu
if [ -e "$FAKE_GLEN_STATE/oversize" ] && [ "$1" = -sk ]; then
  printf '512001\t%s\n' "$2"
else
  exec "$FAKE_REAL_DU" "$@"
fi
EOF
chmod +x "$DISPATCH_BIN/date" "$DISPATCH_BIN/sleep" "$DISPATCH_BIN/du"

dispatch_fixture() {
  STATE="$WORK/dispatch/$1" CASE_MODE="${2:-review}" CASE_DEADLINE=15m
  CASE_PROVIDER="${3:-cursor}"
  mkdir "$STATE"
  CASE_OUT="$STATE/answer.md"
  printf '100000\n' > "$STATE/clock"
  : > "$STATE/plan.jsonl"
  : > "$STATE/calls.jsonl"
  : > "$STATE/sleeps"
  for job in j0000001 j0000002; do
    jq -cn --arg provider "$CASE_PROVIDER" '
      {type:"message",message:{role:"assistant",model:"fixture-model",provider:$provider,
        usage:{input:2,output:3,cacheRead:4,cacheWrite:5,totalTokens:14,cost:{total:1}}}},
      {type:"message",message:{role:"assistant",model:"fixture-model",provider:$provider,
        usage:{input:7,output:11,cacheRead:13,cacheWrite:17,totalTokens:48,cost:{total:2}}}}
    ' > "$STATE/session-$job.jsonl"
  done
  if [ "$CASE_MODE" = review ]; then ANSWER=$'## omp review\nAPPROVE\nMnemosyne: unavailable'; else ANSWER=$'## Root Cause\nFixture'; fi
  DONE="$(dispatch_envelope completed "$ANSWER" j0000001)"
  DONE2="$(dispatch_envelope completed "$ANSWER" j0000002)"
  RUNNING="$(dispatch_envelope running '' j0000001)"
}
dispatch_envelope() {
  jq -cn --arg status "$1" --arg answer "$2" --arg job "$3" --arg state "$STATE" '
    {v:1,ok:true,data:{status:$status,assistant_text:$answer,command_output:[],
      delivered_as:"prompt",forked:null,session_settled:($status=="completed"),stop_reason:null,
      job:{id:("00000000-0000-7000-8000-00000"+$job[1:]),handle:$job,
        status:(if $status=="completed" then "completed" else "accepted" end),route:"new",
        deliver_as:"auto",delivered_as:"prompt",session_id:null,session_handle:("a"+$job[1:]),
        actual_session_id:null,created_at:"2026-10-30T00:00:00Z",updated_at:"2026-10-30T00:00:00Z",elapsed_ms:0},
      session:{id:null,handle:("a"+$job[1:]),path:($state+"/session-"+$job+".jsonl")},
      replies:(if $status=="completed" then [{v:1,text:"fixture reply",final:true,
        id:"fixture-reply",job_id:$job,request_id:null,session_id:null,tool_call_id:"fixture-call",at:"2026-10-30T00:00:00Z"}] else [] end)}}'
}
dispatch_error() {
  jq -cn --arg code "$1" --arg message "$2" --arg nested "$3" --arg incidental "${4:-}" \
    --argjson job "$(printf '%s' "$DONE" | jq '.data.job')" '
    {v:1,ok:false,error:{code:"job_failed",message:$message,details:{status:"failed",job:($job|.status="failed"),
      forked:null,error:{code:$code,message:$nested},request_id:$incidental}}}'
}
dispatch_step() {
  local extra='{}'
  [ "$#" -lt 4 ] || extra="$4"
  jq -cn --arg command "$1" --argjson code "$2" --argjson envelope "$3" --argjson extra "$extra" \
    '{command:$command,exit:$code,envelope:$envelope} + $extra' >> "$STATE/plan.jsonl"
}
dispatch_command() {
  local mode="$1" mode_args=(--uncommitted)
  shift
  [ "$mode" = review ] || mode_args=(--brief "$WORK/brief.md")
  env HOME="$WORK/home" PI_CONFIG_DIR=.omp GLEN_OMP_ENGINE=on \
    FAKE_GLEN_STATE="$STATE" PATH="$DISPATCH_BIN:$PATH" \
    GLEN_BIN="$TEST_DIR/support/glen" /bin/bash "$ENGINE" "$mode" --repo "$DISPATCH_SRC" \
    "${mode_args[@]}" --out "$CASE_OUT" --deadline "$CASE_DEADLINE" "$@"
}
dispatch_run() {
  jq -s . "$STATE/plan.jsonl" > "$STATE/steps.json"
  run dispatch_command "$CASE_MODE" "$@"
  assert "dispatch $(basename "$STATE") emits one public result" jq -es 'length==1 and .[0].v==1 and .[0].engine=="omp"' "$STDOUT"
}
assert_tree() {
  local label="$1" expected="$2" tree
  tree="$(jq -r '.run_dir + "/tree"' "$STDOUT")"
  if [ "$expected" = retained ]; then assert "$label" test -d "$tree"; else assert "$label" test ! -e "$tree"; fi
}
await_dispatch_wait() {
  local attempt
  for ((attempt=0; attempt<500; attempt++)); do
    [ ! -e "$STATE/waiting" ] || return 0
    kill -0 "$ENGINE_PID" 2>/dev/null || break
    /bin/sleep 0.02
  done
  return 1
}

# Read FILE.json while wait is actually blocked, for both a completed and failed job.
for terminal in ok failed; do
  dispatch_fixture "wait-$terminal"
  dispatch_step send 10 "$RUNNING"
  dispatch_step wait 10 "$RUNNING" '{"job":"j0000001"}'
  if [ "$terminal" = ok ]; then
    dispatch_step wait 0 "$DONE" '{"job":"j0000001","barrier":true}'
  else
    dispatch_step wait 7 "$(dispatch_error omp_error 'job failed' 'provider failed')" '{"job":"j0000001","barrier":true}'
  fi
  jq -s . "$STATE/plan.jsonl" > "$STATE/steps.json"
  dispatch_command "$CASE_MODE" > "$STDOUT" 2> "$STDERR" &
  ENGINE_PID=$!
  assert "dispatch $terminal reaches blocked wait" await_dispatch_wait
  assert "dispatch $terminal is alive at running checkpoint" kill -0 "$ENGINE_PID"
  assert "dispatch $terminal persists running recovery metadata mid-flight" jq -e '
    .status=="running" and .job=="j0000001" and (.run_dir|type=="string") and
    .glen_home==(.run_dir+"/glen-home")' "$CASE_OUT.json"
  checkpoint_tree="$(jq -r '.run_dir + "/tree"' "$CASE_OUT.json")"
  assert "dispatch $terminal tree exists during wait" test -d "$checkpoint_tree"
  : > "$STATE/release"
  if wait "$ENGINE_PID"; then CODE=0; else CODE="$?"; fi
  assert "dispatch $terminal emits one public result" jq -es 'length==1 and .[0].v==1 and .[0].engine=="omp"' "$STDOUT"
  if [ "$terminal" = ok ]; then expected=0; else expected=4; fi
  assert_exit "dispatch $terminal terminal exit" "$expected"
  assert_eq "dispatch $terminal terminal status" "$(jq -r .status "$STDOUT")" "$terminal"
  assert_eq "dispatch $terminal send and repeated wait count" "$(cat "$STATE/count")" 3
  assert_tree "dispatch $terminal cleans terminal tree" removed
  if [ "$terminal" = ok ]; then
    assert_eq 'dispatch captures exact final answer' "$(cat "$CASE_OUT")" "$ANSWER"
    assert 'dispatch sums session usage and replies' jq -e '.replies==1 and .usage=={input:9,output:14,cacheRead:17,cacheWrite:22,totalTokens:62,cost_total:3}' "$STDOUT"
  fi
done

dispatch_fixture running-no-job
dispatch_step send 10 "$(printf '%s' "$RUNNING" | jq '.data.job=null')"
dispatch_run
assert_exit 'running without job fails rather than claiming timeout' 4
assert_contains 'running without job reason' 'running glen result has no job handle' "$STDOUT"
assert 'running without job cannot claim recoverability' jq -e '.status=="failed" and .job==null' "$CASE_OUT.json"
assert_tree 'running without job cleans tree' removed

for with_job in yes no; do
  dispatch_fixture "job-timeout-$with_job"
  error="$(dispatch_error timeout 'job failed' 'deadline exceeded')"
  [ "$with_job" = yes ] || error="$(printf '%s' "$error" | jq '.error.details.job=null')"
  dispatch_step send 7 "$error"
  dispatch_run
  if [ "$with_job" = yes ]; then
    assert_exit 'glen timeout maps to engine timeout' 5
    assert 'timeout has a recoverable job handle' jq -e '.status=="timeout" and .job=="j0000001"' "$CASE_OUT.json"
    assert_tree 'timeout preserves tree' retained
  else
    assert_exit 'glen timeout without job fails' 4
    assert 'timeout without job never claims timeout recovery' jq -e '.status=="failed" and .job==null' "$CASE_OUT.json"
    assert_contains 'timeout without job reason' 'job timeout result has no job handle' "$STDOUT"
    assert_tree 'timeout without job cleans tree' removed
  fi
done

for location in top nested code incidental; do
  dispatch_fixture "capacity-$location"
  case "$location" in
    top) error="$(dispatch_error omp_error 'rate limit exceeded' 'provider failed')" ;;
    nested) error="$(dispatch_error omp_error 'job failed' 'rate limit exceeded')" ;;
    code) error="$(dispatch_error overloaded 'job failed' 'provider failed')" ;;
    incidental) error="$(dispatch_error omp_error 'job failed' 'provider failed' request-429-capacity-id)" ;;
  esac
  dispatch_step send 7 "$error"
  [ "$location" = incidental ] || dispatch_step send 0 "$DONE2"
  dispatch_run
  if [ "$location" = incidental ]; then
    assert_exit 'incidental capacity text does not retry' 4
    assert_eq 'incidental capacity text sends once' "$(cat "$STATE/count")" 1
    assert 'incidental capacity text never sleeps' test ! -s "$STATE/sleeps"
  else
    assert_exit "capacity $location retries successfully" 0
    assert_eq "capacity $location sends twice" "$(cat "$STATE/count")" 2
    assert_eq "capacity $location waits exactly once" "$(cat "$STATE/sleeps")" 60
    assert "capacity $location resends new session with reduced budget" jq -es '
      all(.[]; .argv[2:6]==["send","@new","-","--cwd"]) and
      .[0].job_timeout=="900s" and .[1].job_timeout=="840s"' "$STATE/calls.jsonl"
    assert_eq "capacity $location final job is new job" "$(jq -r .job "$STDOUT")" j0000002
    assert_contains "capacity $location final session is new session" session-j0000002.jsonl "$STDOUT"
  fi
done
for limit in budget once; do
  dispatch_fixture "capacity-limit-$limit"
  error="$(dispatch_error omp_error 'rate limit exceeded' 'provider failed')"
  dispatch_step send 7 "$error"
  if [ "$limit" = budget ]; then CASE_DEADLINE=299s; else dispatch_step send 7 "$error"; fi
  dispatch_run
  assert_exit "capacity $limit limit fails dispatch" 4
  if [ "$limit" = budget ]; then expected=1; else expected=2; fi
  assert_eq "capacity $limit limit caps send count" "$(cat "$STATE/count")" "$expected"
  if [ "$limit" = budget ]; then assert 'short budget never sleeps' test ! -s "$STATE/sleeps"; else assert_eq 'repeated capacity failure sleeps once' "$(cat "$STATE/sleeps")" 60; fi
done

for spill in valid missing invalid; do
  dispatch_fixture "spill-$spill"
  if [ "$spill" = valid ]; then
    dispatch_step send 10 "$RUNNING" '{"spill":"valid"}'
    dispatch_step wait 0 "$DONE" '{"spill":"valid","job":"j0000001"}'
  else
    dispatch_step send 0 "$DONE" "{\"spill\":\"$spill\"}"
  fi
  dispatch_run
  if [ "$spill" = valid ]; then
    assert_exit 'spilled envelope loads successfully' 0
    assert_eq 'spilled envelope answer retained' "$(cat "$CASE_OUT")" "$ANSWER"
  else
    assert_exit "$spill spilled envelope fails closed" 4
    assert_contains "$spill spilled envelope reason" 'invalid glen envelope' "$STDOUT"
    assert_tree "$spill spilled envelope cleans tree" removed
  fi
done

for provider in cursor anthropic; do
  for keep in no yes; do
    dispatch_fixture "provider-$provider-$keep" review "$provider"
    dispatch_step send 0 "$DONE"
    if [ "$keep" = yes ]; then dispatch_run --keep; else dispatch_run; fi
    if [ "$provider" = cursor ]; then
      assert_exit "cursor provider $keep accepted" 0
    else
      assert_exit "foreign provider $keep rejected" 4
      assert_eq "foreign provider $keep failure reason" "$(jq -r .reason "$STDOUT")" 'ran on anthropic, not cursor'
    fi
    if [ "$keep" = yes ]; then expected=retained; else expected=removed; fi
    assert_tree "provider $provider keep=$keep cleanup" "$expected"
  done
done
for shape in review-ok review-heading review-tail debug-ok debug-heading; do
  case "$shape" in debug-*) mode=debug ;; *) mode=review ;; esac
  dispatch_fixture "shape-$shape" "$mode"
  case "$shape" in
    review-heading) ANSWER=$'APPROVE\nMnemosyne: unavailable' ;;
    review-tail) ANSWER=$'## omp review\nAPPROVE' ;;
    debug-heading) ANSWER='Fixture without a root cause heading' ;;
  esac
  dispatch_step send 0 "$(dispatch_envelope completed "$ANSWER" j0000001)"
  dispatch_run
  assert_exit "$shape completed execution exits zero" 0
  assert_eq "$shape always preserves answer text" "$(cat "$CASE_OUT")" "$ANSWER"
  case "$shape" in
    *-ok) assert_eq "$shape has a vote" "$(jq -r .status "$STDOUT")" ok ;;
    *) assert 'malformed output gets no vote' jq -e '.status=="failed" and .reason=="malformed output"' "$CASE_OUT.json" ;;
  esac
  assert_tree "$shape terminal tree cleanup" removed
done

for phase in send wait; do
  dispatch_fixture "interrupted-$phase"
  [ "$phase" != wait ] || dispatch_step send 10 "$RUNNING"
  dispatch_step "$phase" 0 '{}' '{"signal":"TERM"}'
  dispatch_run
  if [ "$phase" = send ]; then
    assert_exit 'interruption before job is failure' 4
    assert 'interruption before job is not recoverable timeout' jq -e '.status=="failed" and .job==null' "$CASE_OUT.json"
    assert_contains 'interruption before job reason' 'before any job handle was obtained' "$STDOUT"
    assert_tree 'interruption before job cleans tree' removed
  else
    assert_exit 'interruption after job is timeout' 5
    assert 'interruption timeout names known job' jq -e '.status=="timeout" and .job=="j0000001"' "$CASE_OUT.json"
    assert_tree 'interruption after job preserves tree' retained
  fi
done
for advance in 75 76; do
  dispatch_fixture "deadline-$advance"
  CASE_DEADLINE=1s
  dispatch_step send 10 "$RUNNING" "{\"advance\":$advance}"
  [ "$advance" -ne 75 ] || dispatch_step wait 0 "$DONE" '{"job":"j0000001"}'
  dispatch_run
  if [ "$advance" -eq 75 ]; then
    assert_exit 'deadline grace still permits final wait' 0
  else
    assert_exit 'engine deadline expires with known job' 5
    assert_eq 'engine deadline names its own expiration reason' "$(jq -r .reason "$STDOUT")" 'engine deadline exceeded'
    assert_eq 'expired engine deadline never starts wait' "$(cat "$STATE/count")" 1
    assert 'engine deadline timeout has job' jq -e '.status=="timeout" and .job=="j0000001"' "$CASE_OUT.json"
    assert_tree 'engine deadline preserves tree' retained
  fi
done
dispatch_fixture snapshot-skipped
: > "$STATE/oversize"
dispatch_run
assert_exit 'production snapshot skip propagates' 3
assert_eq 'production snapshot skip status' "$(jq -r .status "$STDOUT")" skipped
assert_tree 'production snapshot skip cleans tree' removed
assert 'production snapshot skip never dispatches' test ! -s "$STATE/calls.jsonl"

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
"omp" "login"
o"m"p l'og'in
OMP LoGiN
/usr/local/bin/omp config set a b
env omp logout
command omp login
sudo -u root omp config set a b
nice -n 5 omp update
VAR=value omp config reset a
env VAR=value command nice -n 5 omp login
echo safe || omp login
printf safe | omp logout
(omp login)
OMP_PROFILE=default omp login
OMP_PROFILE='' omp update
PI_PROFILE=default omp logout
PI_PROFILE="" omp config set a b
OMP_PROFILE='' PI_PROFILE=glen-harness omp login
OMP_PROFILE=default PI_PROFILE=glen-harness omp login
OMP_PROFILE=glen-harness env -u OMP_PROFILE omp login
OMP_PROFILE=glen-harness env --unset=OMP_PROFILE omp login
OMP_PROFILE=glen-harness omp --profile default update
PI_PROFILE=glen-harness omp --profile=default config set a b
PI_CODING_AGENT_DIR=/tmp/x omp --profile default login
OMP_PROFILE=glen-harness omp --profile='' login
OMP_PROFILE=glen-harness omp --profile "$GUARD_UNSET_PROFILE" login
PI_CODING_AGENT_DIR=~/.omp/agent glen install-extension
PI_CODING_AGENT_DIR="$HOME/.omp/agent" omp login
PI_CODING_AGENT_DIR=${HOME}/.omp/agent omp config reset a
PI_CODING_AGENT_DIR="$HOME/.omp/other/../agent" glen install-extension
env PI_CODING_AGENT_DIR="~/.omp/agent" omp update
PI_CODING_AGENT_DIR="$GUARD_UNSET_DIR" glen install-extension
omp login --dry-run
omp --dry-run config set a b
omp settings --dry-run
omp gc --apply
omp gc --dry-run --apply
omp auth import fixture.json
omp agents --force
omp settings
omp session unpack fixture.tar
omp gc --apply # --profile glen-harness
omp login # --dry-run --profile glen-harness
echo "$(omp login)"
echo `omp logout`
printf '%s' "$(echo "$(omp login)")"
echo safe > /tmp/fixture; omp 2>/tmp/fixture config set a b
OMP_PROFILE=glen-harness env -i PATH="$PATH" omp login
OMP_PROFILE=glen-harness env --ignore-environment PATH="$PATH" omp login
OMP_PROFILE=glen-harness env -uOMP_PROFILE omp login
PI_CODING_AGENT_DIR=/tmp/x env -u PI_CODING_AGENT_DIR glen install-extension
env OMP_PROFILE=$GUARD_UNSET_PROFILE omp login
env OMP_PROFILE="$GUARD_UNSET_PROFILE" omp login
OMP_PROFILE=glen-harness sudo omp login
OMP_PROFILE=glen-harness omp --profile="$GUARD_UNSET_PROFILE" login
OMP_PROFILE=glen-harness omp --profile="$(printf default)" login
omp "$GUARD_UNSET_VERB" a b
omp config "$GUARD_UNSET_VERB" a b
echo > "$(omp login)"
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
rg -n "omp config set"
git commit -m "omp login"
gh pr create --body "...omp config set..."
echo omp login
printf '%s' 'omp login; glen install-extension'
echo ';' 'omp' 'login'
echo '$(omp login)'
echo '#'; rg -n "omp config set # still an argument"
# omp config set a b
OMP_PROFILE=glen-harness omp update # omp login
env -- OMP_PROFILE=glen-harness omp update
PI_PROFILE=glen-harness omp login
OMP_PROFILE=glen-harness PI_PROFILE=default omp login
OMP_PROFILE=default omp --profile glen-harness login
PI_PROFILE='' omp --profile=glen-harness config set a b
PI_CODING_AGENT_DIR=~/.omp/agent glen --profile glen-harness install-extension
PI_CODING_AGENT_DIR=/tmp/x omp config set a b
PI_CODING_AGENT_DIR="$TMPDIR/omp-fixture" omp update
env 'PI_CODING_AGENT_DIR=/tmp/x' glen install-extension
env -u OMP_PROFILE OMP_PROFILE=glen-harness omp login
command -v omp
command -V omp
command -- omp --profile glen-harness login
sudo -u root nice -n 5 omp --profile glen-harness config set a b
glen install-extension --dry-run --uninstall
glen --profile default install-extension --dry-run
omp gc
omp gc --dry-run
omp auth status
omp agents
omp session pack fixture
omp --profile glen-harness gc --apply
omp --profile glen-harness auth import fixture.json
omp --profile glen-harness agents --force
omp --profile glen-harness settings
omp --profile glen-harness session unpack fixture.tar
OMP_PROFILE=glen-harness sudo -E omp login
OMP_PROFILE=glen-harness env -i PATH="$PATH" PI_PROFILE=glen-harness omp login
omp config
omp --profile glen-harness "$GUARD_UNSET_VERB" a b
EOF

# Multiline boundaries must be checked after joining continuations and removing
# comments; a quoted separator or comment cannot grant a profile exemption.
run_hook $'omp \\\nconfig \\\nset a b'
assert_exit 'guard joins backslash-newline before matching' 2
run_hook $'echo safe # omp --profile glen-harness\nomp login'
assert_exit 'guard ignores comment profile and checks next line' 2
run_hook $'omp login # --profile \\\nglen-harness'
assert_exit 'guard joins continuation before stripping comments' 2
run_hook $'rg -n "omp \\\nconfig set"'
assert_exit 'guard preserves quoted argument across continuation' 0
run_hook $'omp --profile \\\nglen-harness update'
assert_exit 'guard recognizes continued explicit profile' 0

mkdir "$WORK/parser-failure"
printf '#!/bin/sh\nexit 127\n' > "$WORK/parser-failure/python3"
chmod +x "$WORK/parser-failure/python3"
(
  export PATH="$WORK/parser-failure:$PATH"
  run_hook 'omp login'
  printf '%s\n' "$CODE" > "$WORK/parser-code"
)
CODE="$(cat "$WORK/parser-code")"
assert_exit 'guard parser startup failure blocks command' 2
assert_contains 'guard parser startup failure names problem' 'Cannot run the omp profile guard parser' "$STDERR"

printf '%s assertions, %s failures\n' "$CASES" "$FAILURES"
[ "$FAILURES" -eq 0 ]
