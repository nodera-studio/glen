#!/usr/bin/env bash
# W9 smoke: scratch repos/homes and CLI stubs, plus gated macOS sandbox checks.
set -euo pipefail
umask 077

TEST_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$TEST_DIR/../../.." && pwd -P)"
ENGINE="$REPO_ROOT/.claude/scripts/omp-engine.sh"
SANDBOX="$REPO_ROOT/.claude/omp/omp-sandboxed.sh"
LINUX_SANDBOX="$REPO_ROOT/.claude/omp/omp-sandboxed-linux.sh"
# Strict ancestor checks reject /tmp (01777), even above a private child. Linux
# fixtures use the real user's home as scratch, separate from every fake HOME.
# Darwin's normal TMPDIR already has a non-writable physical ancestor chain.
if [ "$(uname -s)" = Darwin ]; then smoke_parent="${TMPDIR:-/tmp}"; else smoke_parent="$HOME"; fi
WORK="$(mktemp -d "$smoke_parent/glen-omp-smoke.XXXXXX")"
WORK="$(cd "$WORK" && pwd -P)"
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
    "$sandbox_base/home/.omp/profiles/glen-harness/agent/extensions" \
    "$sandbox_base/home/.omp/profiles/glen-harness/agent/skills" \
    "$sandbox_base/home/.omp/profiles/glen-harness/agent/sessions" \
    "$sandbox_base/home/.omp/profiles/glen-harness/logs" \
    "$sandbox_base/home/.omp/profiles/glen-harness/cache" \
    "$sandbox_base/home/.omp/profiles/glen-harness/run" \
    "$sandbox_base/home/.omp/natives" "$sandbox_base/run" "$sandbox_base/bin"
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
  for name in config.yml config.yml.lock mcp.json extensions/x.ts skills/x.md unknown-future-file; do
    printf 'read-only fixture\n' > "$sandbox_base/home/.omp/profiles/glen-harness/agent/$name"
  done
  printf 'native fixture\n' > "$sandbox_base/home/.omp/natives/x"
  # Fix cycle 6, HIGH-1: shared and profile daemon state the child must
  # neither read nor plant, next to the lease files it needs.
  mkdir -p "$sandbox_base/home/.omp/run/daemons/abc" "$sandbox_base/home/.omp/run/session-owners" \
    "$sandbox_base/home/.omp/profiles/glen-harness/run/daemons/0123456789abcdef/daemons/x" \
    "$sandbox_base/home/.omp/profiles/glen-harness/agent/sessions/other-slug" "$sandbox_base/run/staged/daemons/x"
  printf 'token fixture\n' > "$sandbox_base/home/.omp/run/daemons/abc/broker.token"
  printf 'spec fixture\n' > "$sandbox_base/home/.omp/profiles/glen-harness/run/daemons/0123456789abcdef/daemons/x/spec.json"
  printf 'planted spec\n' > "$sandbox_base/run/staged/daemons/x/spec.json"
  printf 'session fixture\n' > "$sandbox_base/home/.omp/profiles/glen-harness/agent/sessions/other-slug/s.jsonl"

  # Prewarm and sandboxed startup must resolve the same HOME-based native cache.
  # Clear caller overrides for both, rather than warming a separate XDG location.
  fixture_omp_env() {
    env -u CLAUDE_CONFIG_DIR -u PI_PY -u PI_JS -u PI_CONFIG_FILES \
      -u PI_CODING_AGENT_DIR -u OMP_CODING_AGENT_DIR -u OMP_PROFILE -u PI_PROFILE \
      -u XDG_STATE_HOME -u XDG_CONFIG_HOME -u XDG_CACHE_HOME -u XDG_DATA_HOME \
      -u PI_NATIVES_DIR LC_ALL=C HOME="$sandbox_home" PI_CONFIG_DIR=.omp "$@"
  }
  sandbox_call() (
    local binary="$1"
    shift
    # Like omp-engine.sh, give the child physical cwd and file arguments.
    cd "$sandbox_base/run" || exit 1
    fixture_omp_env OMP_REAL_BIN="$binary" \
      OMP_SANDBOX_RUN_ROOT="$sandbox_run" \
      OMP_PROFILE_AGENT_DIR="${sandbox_agent:-$sandbox_home/.omp/profiles/glen-harness/agent}" \
      /bin/bash "$SANDBOX" "$@"
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
  assert_contains 'sandbox canonicalizes HOME' "(subpath \"$sandbox_base/home/.omp/profiles/glen-harness/agent\")" "$generated_profile"
  assert_contains 'sandbox resolves executable symlink chain' "(literal \"$(cd /bin && pwd -P)/cat\")" "$generated_profile"
  assert 'sandbox profile contains no unresolved fixture alias' test "$(grep -Fc sandbox-alias "$generated_profile" || true)" -eq 0
  # An opening quote followed by /var/ must never survive physical resolution.
  assert 'sandbox profile contains no logical /var literal' test "$(grep -Fc '\"/var/' "$generated_profile" || true)" -eq 0
  # Fix cycle 4, item 2: a named-entry write allowlist, never a broad subpath
  # grant over the agent directory. SQLite's directory-level needs (WAL/journal/
  # lock) are met by the state-database regex below, not a subpath grant.
  sed -n '/^(allow file-write\*/,/^)/p' "$generated_profile" > "$WORK/sandbox-writes"
  sed -n '/^(deny file-write\*/,/^)/p' "$generated_profile" > "$WORK/sandbox-write-denies"
  assert 'sandbox agent directory has no broad write subpath grant' test \
    "$(grep -Fc "(subpath \"$sandbox_base/home/.omp/profiles/glen-harness/agent\")" "$WORK/sandbox-writes" || true)" -eq 0
  # Escape the full fixture path the same way sbpl_regex_literal does (every
  # dot, not just the one before "omp"): $WORK itself contains a literal dot
  # (mktemp's "glen-omp-smoke.XXXXXX"), which the real regex also escapes.
  expected_agent_regex="$(printf '%s' "$sandbox_base/home/.omp/profiles/glen-harness/agent" | sed 's/\./\\./g')"
  assert_contains 'sandbox grants only the named state-database family for SQLite' \
    "(regex #\"^$expected_agent_regex/(agent|models|skill-descriptions)\\.db(-wal|-shm|-journal)?\$\")" \
    "$WORK/sandbox-writes"
  assert_contains 'sandbox denies renaming or unlinking the agent directory itself' \
    "(literal \"$sandbox_base/home/.omp/profiles/glen-harness/agent\")" "$WORK/sandbox-write-denies"
  assert_contains 'sandbox denies renaming or unlinking the profile root itself' \
    "(literal \"$sandbox_base/home/.omp/profiles/glen-harness\")" "$WORK/sandbox-write-denies"
  assert 'sandbox configuration denies follow directory write allow' awk \
    '/^\(allow file-write\*/ { allow=NR } /^\(deny file-write\*/ { deny=NR } END { exit !(allow>0 && deny>allow) }' "$generated_profile"
  assert 'sandbox native cache has no write grant' test "$(grep -Fc '/.omp/natives' "$WORK/sandbox-writes" || true)" -eq 0
  for name in config.yml config.yml.lock mcp.json; do
    assert_contains "sandbox denies writes to protected file $name" \
      "(literal \"$sandbox_base/home/.omp/profiles/glen-harness/agent/$name\")" "$WORK/sandbox-write-denies"
  done
  for name in extensions skills unknown-future-file; do
    assert_contains "sandbox denies writes to protected entry $name" \
      "(subpath \"$sandbox_base/home/.omp/profiles/glen-harness/agent/$name\")" "$WORK/sandbox-write-denies"
  done
  for path in agent/sessions logs cache run; do
    assert_contains "sandbox permits runtime subtree $path" \
      "(subpath \"$sandbox_base/home/.omp/profiles/glen-harness/$path\")" "$WORK/sandbox-writes"
  done
  # Fix cycle 6, HIGH-1: only the lease directory of shared ~/.omp/run.
  assert 'sandbox grants no read or write on all of shared ~/.omp/run' test \
    "$(grep -Fc "(subpath \"$sandbox_base/home/.omp/run\")" "$generated_profile" || true)" -eq 0
  assert_contains 'sandbox grants writes to session-owner leases' \
    "(subpath \"$sandbox_base/home/.omp/run/session-owners\")" "$WORK/sandbox-writes"
  assert_contains 'sandbox denies writes to profile daemon records' \
    "(subpath \"$sandbox_base/home/.omp/profiles/glen-harness/run/daemons\")" "$WORK/sandbox-write-denies"
  assert_contains 'sandbox denies reads of profile daemon records' \
    "(deny file-read* (subpath \"$sandbox_base/home/.omp/profiles/glen-harness/run/daemons\"))" "$generated_profile"
  assert 'sandbox without an omp binary grants no daemon scope' test \
    "$(grep -Ec '/run/daemons/[0-9a-f]{16}/clients' "$generated_profile" || true)" -eq 0
  assert_contains 'sandbox denies removing a session directory itself' \
    "(deny file-write-unlink (regex #\"^$expected_agent_regex/sessions/[^/]+\$\"))" "$generated_profile"
  assert_contains 'sandbox maps no code from the run root' \
    "(deny file-map-executable (subpath \"$sandbox_base/run\"))" "$generated_profile"
  assert_contains 'sandbox outbound restricted to TCP 443' '(allow network-outbound (remote tcp "*:443"))' "$generated_profile"
  assert 'sandbox outbound grants no bare IP filter' test "$(grep -Fc '(allow network-outbound (remote ip))' "$generated_profile" || true)" -eq 0
  assert_contains 'sandbox explicitly permits DNS Unix socket' '(allow network-outbound (literal "/private/var/run/mDNSResponder"))' "$generated_profile"
  assert 'sandbox has no blanket outbound grant' test "$(grep -Fxc '(allow network-outbound)' "$generated_profile" || true)" -eq 0
  assert 'sandbox grants no mach-lookup at all (S5)' test "$(grep -Fc '(allow mach-lookup)' "$generated_profile" || true)" -eq 0
  assert 'sandbox restricts process-exec to the resolved binary (S5)' test \
    "$(grep -Fc '(allow process-exec* (literal "'"$(cd /bin && pwd -P)"'/cat"))' "$generated_profile" || true)" -eq 1
  assert 'sandbox grants no blanket process-exec*' test "$(grep -Fxc '(allow process-exec*)' "$generated_profile" || true)" -eq 0

  mkdir -p "$sandbox_base/home/custom-omp/profiles/glen-harness/agent"
  sandbox_agent="$sandbox_alias/home/custom-omp/profiles/glen-harness/agent"
  run_sandbox sandbox_call /bin/cat "$sandbox_base/run/readable"
  custom_profile="$(grep -Fl -- "$sandbox_base/home/custom-omp/profiles/glen-harness/agent" "$sandbox_base/run"/omp-sandbox.*)"
  # Appears twice: the file-read* grant, and the explanatory comment above
  # the deny stanza that names the directory it protects from being renamed.
  assert 'sandbox explicit profile path is canonical for reads' test "$(grep -Fc "(subpath \"$sandbox_base/home/custom-omp/profiles/glen-harness/agent\")" "$custom_profile")" -eq 2
  assert_contains 'sandbox custom profile denies configuration writes' "(literal \"$sandbox_base/home/custom-omp/profiles/glen-harness/agent/config.yml\")" "$custom_profile"
  assert_contains 'sandbox custom profile includes sibling logs' "(subpath \"$sandbox_base/home/custom-omp/profiles/glen-harness/logs\")" "$custom_profile"
  assert 'sandbox explicit profile does not admit old profile root' test "$(grep -Fc '.omp/profiles/glen-harness' "$custom_profile" || true)" -eq 0
  unset sandbox_agent

  # Fix cycle 5, decision 2: the Mnemosyne MCP's own URL derives a loopback
  # outbound rule. A loopback host on a non-443 port is allowed; a
  # non-loopback, non-443 host refuses instead of granting it outbound.
  mkdir -p "$sandbox_base/home/mnemosyne-ok/profiles/glen-harness/agent"
  printf '%s
' '{"mcpServers":{"mnemosyne":{"type":"http","url":"http://127.0.0.1:9191/mcp","headers":{"Authorization":"Bearer secret-token-never-logged"}}}}'     > "$sandbox_base/home/mnemosyne-ok/profiles/glen-harness/agent/mcp.json"
  sandbox_agent="$sandbox_alias/home/mnemosyne-ok/profiles/glen-harness/agent"
  run_sandbox sandbox_call /bin/cat "$sandbox_base/run/readable"
  mnemosyne_profile="$(grep -Fl -- "$sandbox_base/home/mnemosyne-ok/profiles/glen-harness/agent" "$sandbox_base/run"/omp-sandbox.*)"
  assert_exit 'sandbox still runs with a loopback Mnemosyne MCP entry' 0
  assert_contains 'sandbox derives the Mnemosyne loopback outbound rule'     '(allow network-outbound (remote tcp "localhost:9191"))' "$mnemosyne_profile"
  assert 'sandbox never carries the Mnemosyne token into the profile' test     "$(grep -Fc 'secret-token-never-logged' "$mnemosyne_profile" || true)" -eq 0
  unset sandbox_agent

  mkdir -p "$sandbox_base/home/mnemosyne-remote/profiles/glen-harness/agent"
  printf '%s
' '{"mcpServers":{"mnemosyne":{"type":"http","url":"http://mnemosyne.example.com:9191/mcp"}}}'     > "$sandbox_base/home/mnemosyne-remote/profiles/glen-harness/agent/mcp.json"
  sandbox_agent="$sandbox_alias/home/mnemosyne-remote/profiles/glen-harness/agent"
  run_sandbox sandbox_call /bin/cat "$sandbox_base/run/readable"
  assert_exit 'sandbox refuses a non-loopback, non-443 Mnemosyne MCP host' 1
  assert_contains 'non-loopback refusal names the reason' 'neither loopback nor on 443' "$STDERR"
  unset sandbox_agent

  # Fix cycle 6 (Claude LOW-2 / Codex P2): bracketed IPv6 loopback and any
  # letter case derive the rule; an unparsable URL says so, without the URL.
  for mnemosyne_case in 'ipv6 http://[::1]:9192/mcp 9192' 'upper HTTP://LocalHost:9193/mcp 9193' 'userinfo http://u@127.0.0.1:9194/mcp -'; do
    # shellcheck disable=SC2086 # intentional word splitting of the case.
    set -- $mnemosyne_case
    mkdir -p "$sandbox_base/home/mnemosyne-$1/profiles/glen-harness/agent"
    jq -cn --arg url "$2" '{mcpServers:{mnemosyne:{type:"http",url:$url}}}' \
      > "$sandbox_base/home/mnemosyne-$1/profiles/glen-harness/agent/mcp.json"
    sandbox_agent="$sandbox_alias/home/mnemosyne-$1/profiles/glen-harness/agent"
    run_sandbox sandbox_call /bin/cat "$sandbox_base/run/readable"
    case_profile="$(grep -Fl -- "$sandbox_base/home/mnemosyne-$1/profiles/glen-harness/agent" "$sandbox_base/run"/omp-sandbox.*)"
    assert_exit "sandbox runs with a $1 Mnemosyne URL" 0
    if [ "$3" = - ]; then
      assert "sandbox derives no rule from a $1 Mnemosyne URL" test \
        "$(grep -Fc '(allow network-outbound (remote tcp "localhost:' "$case_profile" || true)" -eq 0
      assert_contains "sandbox notes the missing Mnemosyne rule for a $1 URL" 'no Mnemosyne network rule derived' "$STDERR"
      assert "sandbox note never prints the $1 URL" test "$(grep -Fc '127.0.0.1:9194' "$STDERR" || true)" -eq 0
    else
      assert_contains "sandbox derives the loopback rule from a $1 Mnemosyne URL" \
        "(allow network-outbound (remote tcp \"localhost:$3\"))" "$case_profile"
    fi
    unset sandbox_agent
  done

  # Fix cycle 5 decision 5 / cycle 6 LOW-3: a path an SBPL regex literal
  # cannot carry is refused before any policy is written.
  for odd_case in 'quote "' 'backslash \'; do
    odd_name="${odd_case#* }"
    mkdir -p "$sandbox_base/home/odd${odd_name}dir/profiles/glen-harness/agent"
    sandbox_agent="$sandbox_alias/home/odd${odd_name}dir/profiles/glen-harness/agent"
    run_sandbox sandbox_call /bin/cat "$sandbox_base/run/readable"
    assert_exit "sandbox refuses an agent path with a ${odd_case% *}" 1
    assert_contains "sandbox ${odd_case% *} refusal names the reason" 'SBPL regex literal cannot carry' "$STDERR"
    assert "sandbox ${odd_case% *} refusal runs nothing" test ! -s "$STDOUT"
    unset sandbox_agent
  done

  # Fix cycle 6, MED-3: a fresh profile without its runtime directories gets
  # them from the launcher; a symlinked one is refused before omp starts.
  mkdir -p "$sandbox_base/home/fresh/profiles/glen-harness/agent"
  sandbox_agent="$sandbox_alias/home/fresh/profiles/glen-harness/agent"
  run_sandbox sandbox_call /bin/cat "$sandbox_base/run/readable"
  assert_exit 'sandbox starts on a fresh profile without runtime directories' 0
  for path in agent/sessions logs cache run run/daemons; do
    assert "launcher creates fresh-profile $path" test -d "$sandbox_base/home/fresh/profiles/glen-harness/$path" -a ! -L "$sandbox_base/home/fresh/profiles/glen-harness/$path"
  done
  assert 'launcher creates the shared lease directory' test -d "$sandbox_base/home/.omp/run/session-owners"
  assert 'launcher creates the private TMPDIR in the run root' test -d "$sandbox_base/run/tmp"
  if [ "$sandbox_runtime" -eq 1 ]; then
    run_sandbox sandbox_call /bin/mkdir "$sandbox_base/home/fresh/profiles/glen-harness/agent/sessions/new-slug"
    assert_exit 'sandbox child can create a session directory on a fresh profile' 0
  fi
  unset sandbox_agent
  mkdir -p "$sandbox_base/home/symlinked/profiles/glen-harness/agent" "$sandbox_base/elsewhere-logs"
  ln -s "$sandbox_base/elsewhere-logs" "$sandbox_base/home/symlinked/profiles/glen-harness/logs"
  sandbox_agent="$sandbox_alias/home/symlinked/profiles/glen-harness/agent"
  run_sandbox sandbox_call /bin/cat "$sandbox_base/run/readable"
  assert_exit 'sandbox refuses a symlinked runtime directory' 1
  assert_contains 'symlinked runtime directory refusal names the reason' 'refusing symlinked runtime directory' "$STDERR"
  assert 'symlinked runtime directory refusal runs nothing' test ! -s "$STDOUT"
  unset sandbox_agent

  # Verify the SQLite CLI/query itself even when nested Seatbelt is unavailable.
  # The sandboxed cases below must perform these same real transactions.
  for journal in delete wal; do
    run /usr/bin/sqlite3 -batch -bail "$WORK/sqlite-control-$journal.db" \
      "PRAGMA journal_mode=$journal; CREATE TABLE t(x INTEGER); INSERT INTO t VALUES(42); SELECT x FROM t;"
    assert_exit "unconfined SQLite $journal control succeeds" 0
    assert_eq "unconfined SQLite $journal control returns inserted row" "$(cat "$STDOUT")" "$(printf '%s\n42' "$journal")"
  done

  # Use an interpreter whose executable/stdlib the existing policy can read.
  # /usr/bin/python3 may forward to Xcode under unlisted /Applications instead.
  socket_python=''
  for candidate in /Library/Developer/CommandLineTools/usr/bin/python3 /usr/bin/python3 "$(command -v python3 || true)"; do
    if [ -x "$candidate" ] && "$candidate" -I -c '
import os, socket, sys
paths = (sys.executable, sys.base_prefix, socket.__file__)
sys.exit(0 if all(os.path.realpath(p).startswith(("/usr/", "/System/", "/Library/")) for p in paths) else 1)
' >/dev/null 2>&1; then
      socket_python="$candidate"
      break
    fi
  done
  assert 'Unix socket probe has Python in permitted system paths' test -n "$socket_python"
  socket_probe="$sandbox_base/run/unix-connect-probe"
  if [ -n "$socket_python" ]; then
    printf '#!%s -I\n' "$socket_python" > "$socket_probe"
    cat >> "$socket_probe" <<'PY'
import socket
import sys

try:
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(2)
        client.connect(sys.argv[1])
except OSError as error:
    print(str(error), file=sys.stderr)
    raise SystemExit(1)
print("glen-unix-connected")
PY
    chmod +x "$socket_probe"
    # This real probe check needs neither a listening socket nor sandbox_apply.
    # Keep below Darwin's sockaddr_un path limit, including physical TMPDIR.
    run fixture_omp_env "$socket_probe" "$sandbox_base/no.sock"
    assert_exit 'unconfined Unix socket probe reports failed connect' 1
    assert_eq 'unconfined Unix socket probe prints exact OSError' "$(cat "$STDERR")" '[Errno 2] No such file or directory'
    assert 'failed Unix socket probe prints no success marker' test ! -s "$STDOUT"
  fi

  if [ "$sandbox_runtime" -eq 1 ]; then
    # process-exec* is restricted to the literal resolved binary (S5), so this
    # probe uses printenv directly instead of /bin/sh -c (sh re-execs /bin/bash
    # internally on this platform, which the narrowed grant would deny).
    run_sandbox sandbox_call /usr/bin/printenv HOME
    assert_eq 'sandbox child inherits canonical HOME' "$(cat "$STDOUT")" "$sandbox_base/home"
    for denied in home/.ssh/known_hosts home/glen-canary.txt home/.omp/agent/config.yml AGENTS.md; do
      run_sandbox sandbox_call /bin/cat "$sandbox_base/$denied"
      assert "sandbox denies outside read $denied" test "$CODE" -ne 0
      # Match cat's file-specific denial, not sandbox_apply failing before exec.
      assert_contains "sandbox outside read is EPERM $denied" "cat: $sandbox_base/$denied: Operation not permitted" "$STDERR"
    done
    for denied in config.yml config.yml.lock mcp.json extensions/x.ts skills/x.md unknown-future-file; do
      path="$sandbox_base/home/.omp/profiles/glen-harness/agent/$denied"
      run_sandbox sandbox_call /usr/bin/tee "$path" <<< 'must not overwrite'
      assert "sandbox refuses configuration write $denied" test "$CODE" -ne 0
      assert_contains "configuration write denial is EPERM $denied" 'Operation not permitted' "$STDERR"
      assert_eq "configuration write denial preserves $denied" "$(cat "$path")" 'read-only fixture'
    done
    run_sandbox sandbox_call /usr/bin/tee "$sandbox_base/home/.omp/natives/x" <<< 'must not overwrite'
    assert 'sandbox refuses native-cache write' test "$CODE" -ne 0
    assert_contains 'native-cache write denial is EPERM' 'Operation not permitted' "$STDERR"
    assert_eq 'native-cache write denial preserves file' "$(cat "$sandbox_base/home/.omp/natives/x")" 'native fixture'
    # Exercise SQLite itself before tee puts dummy bytes into these DB fixtures.
    # Rollback journals and WAL both require more than opening literal DB paths.
    for database in agent.db models.db skill-descriptions.db; do
      for journal in delete wal; do
        run_sandbox sandbox_call /usr/bin/sqlite3 -batch -bail \
          "$sandbox_base/home/.omp/profiles/glen-harness/agent/$database" \
          "PRAGMA journal_mode=$journal; CREATE TABLE IF NOT EXISTS t(x INTEGER); DELETE FROM t; INSERT INTO t VALUES(42); SELECT x FROM t;"
        assert_exit "sandbox SQLite $database $journal round-trip succeeds" 0
        assert_eq "sandbox SQLite $database $journal returns inserted row" "$(cat "$STDOUT")" "$(printf '%s\n42' "$journal")"
      done
    done
    for writable in "$sandbox_base/run/writable" \
      "$sandbox_base/home/.omp/profiles/glen-harness/agent/agent.db" \
      "$sandbox_base/home/.omp/profiles/glen-harness/agent/models.db-wal" \
      "$sandbox_base/home/.omp/profiles/glen-harness/agent/skill-descriptions.db-shm" \
      "$sandbox_base/home/.omp/profiles/glen-harness/agent/sessions/writable" \
      "$sandbox_base/home/.omp/profiles/glen-harness/logs/writable" \
      "$sandbox_base/home/.omp/profiles/glen-harness/cache/writable" \
      "$sandbox_base/home/.omp/profiles/glen-harness/run/writable"; do
      run_sandbox sandbox_call /usr/bin/tee "$writable" <<< 'sandbox write'
      assert_exit 'sandbox allows write in permitted directory' 0
      run_sandbox sandbox_call /bin/cat "$writable"
      assert_exit 'sandbox allows read-back in permitted directory' 0
      assert_eq 'sandbox write read-back content' "$(cat "$STDOUT")" 'sandbox write'
    done

    # Fix cycle 5, decision 4 (Claude MED-2): no symlink creation anywhere,
    # and the sessions/logs/cache/run directories cannot be renamed away.
    for planted in       "$sandbox_base/home/.omp/profiles/glen-harness/agent/sessions/evil-link"       "$sandbox_base/home/.omp/profiles/glen-harness/logs/evil-link"; do
      run_sandbox sandbox_call /bin/ln -s "$sandbox_base/home/glen-canary.txt" "$planted"
      assert_exit 'sandbox refuses planting a symlink in a writable directory' 1
      assert 'sandbox refused symlink never actually appears' test ! -L "$planted"
    done
    harness_profile="$sandbox_base/home/.omp/profiles/glen-harness"
    for renamed_pair in "$harness_profile/agent/sessions sessions.bak" "$harness_profile/logs logs.bak" \
      "$harness_profile/cache cache.bak" "$harness_profile/run run.bak" "$harness_profile/agent agent.bak" \
      "$harness_profile profile.bak" "$sandbox_base/home/.omp/run/session-owners owners.bak"; do
      # shellcheck disable=SC2086 # intentional word splitting of the pair.
      set -- $renamed_pair
      run_sandbox sandbox_call /bin/mv "$1" "$2"
      assert_exit "sandbox refuses renaming ${1##*/} away" 1
      assert "sandbox refused rename leaves ${1##*/} in place" test -d "$1"
    done
    # Claude LOW-7: another session's directory cannot be moved out of
    # sessions/, while files inside it stay writable for omp itself.
    run_sandbox sandbox_call /bin/mv "$harness_profile/agent/sessions/other-slug" "$sandbox_base/run/stolen-session"
    assert_exit 'sandbox refuses moving a session directory out of sessions/' 1
    assert 'refused session move leaves the directory in place' test -f "$harness_profile/agent/sessions/other-slug/s.jsonl"
    run_sandbox sandbox_call /usr/bin/tee "$harness_profile/agent/sessions/other-slug/s.jsonl.tmp" <<< 'session write'
    assert_exit 'sandbox still writes files inside a session directory' 0

    # Fix cycle 6, HIGH-1: broker tokens and daemon records stay out of reach;
    # lease files stay writable.
    run_sandbox sandbox_call /bin/cat "$sandbox_base/home/.omp/run/daemons/abc/broker.token"
    assert_exit 'sandbox denies reading a shared broker token' 1
    assert_contains 'shared broker token read is EPERM' 'Operation not permitted' "$STDERR"
    for denied in "$sandbox_base/home/.omp/run/daemons/abc/spec.json" "$sandbox_base/home/.omp/run/planted" \
      "$harness_profile/run/daemons/0123456789abcdef/daemons/x/spec.json" "$harness_profile/run/daemons/planted"; do
      run_sandbox sandbox_call /usr/bin/tee "$denied" <<< 'planted'
      assert "sandbox denies daemon-state write ${denied#"$sandbox_base/home/"}" test "$CODE" -ne 0
    done
    assert_eq 'sandbox leaves the profile daemon spec intact' \
      "$(cat "$harness_profile/run/daemons/0123456789abcdef/daemons/x/spec.json")" 'spec fixture'
    run_sandbox sandbox_call /bin/cat "$harness_profile/run/daemons/0123456789abcdef/daemons/x/spec.json"
    assert_exit 'sandbox denies reading a profile daemon spec' 1
    run_sandbox sandbox_call /bin/mv "$sandbox_base/run/staged" "$harness_profile/run/daemons/fedcba9876543210"
    assert_exit 'sandbox refuses renaming a staged scope into the daemons tree' 1
    assert 'refused staged scope never appears' test ! -e "$harness_profile/run/daemons/fedcba9876543210"
    run_sandbox sandbox_call /usr/bin/tee "$sandbox_base/home/.omp/run/session-owners/fixture.lock" <<< ''
    assert_exit 'sandbox allows writing a session-owner lease' 0

    # Step 11 acceptance root cause: omp's RPC output spool lands in TMPDIR,
    # which must be the writable run root's private tmp/.
    run_sandbox sandbox_call /usr/bin/printenv TMPDIR
    assert_eq 'sandbox child TMPDIR is the run root tmp' "$(cat "$STDOUT")" "$sandbox_base/run/tmp"
    run_sandbox sandbox_call /usr/bin/mktemp -d -t omp-rpc-output
    assert_exit 'sandbox child can create a temp directory in TMPDIR' 0
    assert 'sandbox temp directory lands in the run root tmp' grep -Fq "$sandbox_base/run/tmp/" "$STDOUT"

    if [ -n "$socket_python" ]; then
      # S5 (fix cycle 4) restricts process-exec* to the literal resolved
      # binary and read access no longer covers /Library beyond Bun's own
      # cache, so the sandboxed leg of this probe cannot use python3 (it
      # dlopens its own CommandLineTools framework, a separate file-read
      # outside any grant). Perl has no such dependency and is already
      # covered by the existing subpath "/usr" read grant. The unconfined
      # control leg is unaffected and still uses the existing Python probe.
      sandbox_probe_pl="$sandbox_base/run/sockconnect.pl"
      cat > "$sandbox_probe_pl" <<'PL'
use strict;
use warnings;
use Socket qw(PF_UNIX SOCK_STREAM sockaddr_un);
my ($path) = @ARGV;
socket(my $sock, PF_UNIX, SOCK_STREAM, 0) or die "socket: $!\n";
if (connect($sock, sockaddr_un($path))) {
  print "glen-unix-connected\n";
} else {
  print STDERR "$!\n";
  exit 1;
}
PL
      run fixture_omp_env "$socket_python" -I - "$SANDBOX" "$sandbox_base" "$socket_probe" "$sandbox_probe_pl" "$WORK/foreign.sock" <<'PY'
import json
import os
import socket
import subprocess
import sys

launcher, base, probe, probe_pl, foreign = sys.argv[1:]
with socket.socket(socket.AF_UNIX) as server:
    server.bind(foreign)
    server.listen(2)
    server.settimeout(2)
    try:
        control = subprocess.run([probe, foreign], stdin=subprocess.DEVNULL,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
        connected = False
        if control.returncode == 0:
            accepted, _ = server.accept()
            accepted.close()
            connected = True
        env = dict(os.environ, HOME=base + "/home", OMP_REAL_BIN="/usr/bin/perl",
                   OMP_SANDBOX_RUN_ROOT=base + "/run",
                   OMP_PROFILE_AGENT_DIR=base + "/home/.omp/profiles/glen-harness/agent")
        child = subprocess.run(["/bin/bash", launcher, probe_pl, foreign], env=env, cwd=base + "/run",
                               stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, timeout=5)
        print(json.dumps({"control_connected": connected, "control_code": control.returncode,
                          "control_output": control.stdout.decode(errors="replace"),
                          "control_stderr": control.stderr.decode(errors="replace"),
                          "code": child.returncode,
                          "stdout": child.stdout.decode(errors="replace"),
                          "stderr": child.stderr.decode(errors="replace")}))
    finally:
        os.unlink(foreign)
PY
      assert_exit 'foreign Unix socket fixture completes' 0
      assert 'foreign Unix socket accepts unconfined control' jq -e '.control_connected == true' "$STDOUT"
      assert 'foreign Unix socket accepts identical unconfined Python probe' jq -e '.control_code == 0 and .control_output == "glen-unix-connected\n" and .control_stderr == ""' "$STDOUT"
      assert 'sandbox refuses foreign Unix socket connection' jq -e '.code == 1 and .stdout == "" and (.stderr | test("Operation not permitted|Permission denied"))' "$STDOUT"
    fi
  fi

  mkdir "$sandbox_base/no-sandbox-bin"
  ln -s "$(command -v uname)" "$sandbox_base/no-sandbox-bin/uname"
  run_sandbox env PATH="$sandbox_base/no-sandbox-bin" HOME="$sandbox_home" \
    OMP_REAL_BIN=/bin/cat OMP_SANDBOX_RUN_ROOT="$sandbox_run" /bin/bash "$SANDBOX" "$sandbox_base/run/readable"
  assert 'sandbox missing executable fails nonzero' test "$CODE" -ne 0
  assert_contains 'sandbox missing executable explains refusal' 'sandbox-exec not on PATH; refusing to run omp unsandboxed' "$STDERR"
  assert 'sandbox missing executable never runs helper' test ! -s "$STDOUT"

  # Model listing must start from a configured profile, as it does after setup.
  # S3 intentionally prevents a sandboxed first run from creating config/MCP.
  real_sandbox_base="$WORK/real-omp-sandbox"
  mkdir -p "$real_sandbox_base/home/.omp/profiles/glen-harness/agent" "$real_sandbox_base/run"
  cat > "$real_sandbox_base/home/.omp/profiles/glen-harness/agent/config.yml" <<'EOF'
modelRoles:
  default: cursor/gemini-4-argon
tools:
  approvalMode: always-ask
mcp:
  enableProjectConfig: false
EOF
  printf '%s\n' '{"mcpServers":{}}' > "$real_sandbox_base/home/.omp/profiles/glen-harness/agent/mcp.json"
  assert_contains 'real omp fixture seeds model configuration before sandbox startup' 'default: cursor/gemini-4-argon' "$real_sandbox_base/home/.omp/profiles/glen-harness/agent/config.yml"
  assert 'real omp fixture seeds MCP without external services' jq -e '.mcpServers == {}' "$real_sandbox_base/home/.omp/profiles/glen-harness/agent/mcp.json"
  cp "$real_sandbox_base/home/.omp/profiles/glen-harness/agent/config.yml" "$real_sandbox_base/config-before"
  cp "$real_sandbox_base/home/.omp/profiles/glen-harness/agent/mcp.json" "$real_sandbox_base/mcp-before"

  if [ -n "$real_omp" ]; then
    # S3 makes natives read-only: populate the scratch HOME's cache unsandboxed
    # first, just as setup does. Keep real omp's DBs separate from helper fixtures.
    sandbox_base="$real_sandbox_base"
    sandbox_home="$sandbox_base/home" sandbox_run="$sandbox_base/run"
    real_omp_prewarm() (
      cd "$sandbox_base/run" || exit 1
      fixture_omp_env "$real_omp" --profile glen-harness toks glen-native-warmup --json
    )
    run real_omp_prewarm
    assert_exit 'real omp unsandboxed native prewarm succeeds' 0
    natives_dir="$sandbox_home/.omp/natives"
    native_addon="$(find "$natives_dir" -type f -name '*.node' -print -quit 2>/dev/null || true)"
    assert 'real omp prewarm populates default HOME native cache' test -n "$native_addon"
    # Always select glen-harness, including --version. Only scratch HOME is used.
    run_sandbox sandbox_call "$real_omp" --profile glen-harness --version
    if [ "$sandbox_runtime" -eq 1 ]; then assert_exit 'sandbox real omp version completes' 0; fi
    real_profiles=("$sandbox_base/run"/omp-sandbox.*)
    sed -n '/^(allow file-read\*/,/^)/p' "${real_profiles[0]}" > "$WORK/real-omp-reads"
    sed -n '/^(allow file-write\*/,/^)/p' "${real_profiles[0]}" > "$WORK/real-omp-writes"
    # Fix cycle 6, HIGH-1: real omp gets exactly its own presence scope,
    # keyed the way omp keys it (wyhash of the physical cwd).
    expected_scope="$(cd / && env -i BUN_BE_BUN=1 HOME=/var/empty GLEN_SCOPE_DIR="$sandbox_base/run" "$real_omp" -e \
      'process.stdout.write(Bun.hash.wyhash(process.env.GLEN_SCOPE_DIR).toString(16).padStart(16, "0"))' < /dev/null 2>/dev/null || true)"
    assert 'real omp scope hash is 16 hex digits' grep -Eqx '[0-9a-f]{16}' <<< "$expected_scope"
    assert_contains 'sandbox grants real omp its own presence directory' \
      "(allow file-write* (subpath \"$sandbox_base/home/.omp/profiles/glen-harness/run/daemons/$expected_scope/clients\"))" "${real_profiles[0]}"
    assert 'sandbox grants real omp exactly one presence directory' test \
      "$(grep -Ec 'allow file-write\* \(subpath "[^"]*/run/daemons/[0-9a-f]{16}/clients"\)\)' "${real_profiles[0]}" || true)" -eq 1
    assert 'launcher precreates the real omp presence directory' \
      test -d "$sandbox_base/home/.omp/profiles/glen-harness/run/daemons/$expected_scope/clients"
    assert_contains 'sandbox reads the same native cache populated by prewarm' "(subpath \"$natives_dir\")" "$WORK/real-omp-reads"
    assert 'sandbox keeps the populated native cache read-only' test "$(grep -Fc "$natives_dir" "$WORK/real-omp-writes" || true)" -eq 0
    if [ "$sandbox_runtime" -eq 1 ]; then
      run_sandbox sandbox_call "$real_omp" --profile glen-harness models --json
      assert_exit 'sandbox real omp models completes without login' 0
      assert 'sandbox real omp leaves seeded config unchanged' cmp -s "$sandbox_home/.omp/profiles/glen-harness/agent/config.yml" "$sandbox_base/config-before"
      assert 'sandbox real omp leaves seeded MCP unchanged' cmp -s "$sandbox_home/.omp/profiles/glen-harness/agent/mcp.json" "$sandbox_base/mcp-before"
    fi
  else
    printf 'NOTE sandbox real omp startup skipped: omp not on PATH\n'
  fi
else
  printf 'NOTE macOS sandbox checks skipped: Darwin with sandbox-exec required\n'
fi

# Inspect Linux bind arguments on either host OS, replacing only uname/bwrap.
# This checks policy construction, not live Linux mount/SQLite compatibility.
linux_policy="$WORK/linux-policy"
mkdir -p "$linux_policy/bin" "$linux_policy/home/.omp/natives" \
  "$linux_policy/home/custom/profiles/glen-harness/agent" "$linux_policy/run"
cat > "$linux_policy/bin/uname" <<'EOF'
#!/bin/sh
printf 'Linux\n'
EOF
cat > "$linux_policy/bin/bwrap" <<'EOF'
#!/bin/sh
jq -cn --args '$ARGS.positional' -- "$@"
EOF
chmod +x "$linux_policy/bin/uname" "$linux_policy/bin/bwrap"
linux_agent="$linux_policy/home/custom/profiles/glen-harness/agent"
printf 'existing SQLite bytes\n' > "$linux_agent/agent.db"
cp "$linux_agent/agent.db" "$linux_policy/database-before"
run env HOME="$linux_policy/home" PATH="$linux_policy/bin:$PATH" \
  OMP_REAL_BIN=/bin/cat OMP_PROFILE_AGENT_DIR="$linux_agent" \
  OMP_SANDBOX_RUN_ROOT="$linux_policy/run" /bin/bash "$LINUX_SANDBOX" --version
assert_exit 'Linux sandbox generates bind list' 0
cp "$STDOUT" "$linux_policy/binds.json"
has_linux_bind() {
  # shellcheck disable=SC2016 # jq receives these variables through --arg.
  jq -e --arg mode "$1" --arg path "$2" '
    . as $args | any(range(0; length - 2); . as $i |
      $args[$i:$i+3] == [$mode,$path,$path])' "$linux_policy/binds.json"
}
assert 'Linux agent baseline is read-only' has_linux_bind --ro-bind "$linux_agent"
assert 'Linux native cache is read-only' has_linux_bind --ro-bind "$linux_policy/home/.omp/natives"
# Exact positive/negative bind checks use the same captured argv, never a fake policy.
if has_linux_bind --bind "$linux_agent" >/dev/null; then writable_agent=yes; else writable_agent=no; fi
assert_eq 'Linux never overlays full agent with writable bind' "$writable_agent" no
if has_linux_bind --bind "$linux_policy/home/.omp/natives" >/dev/null; then writable_natives=yes; else writable_natives=no; fi
assert_eq 'Linux never overlays native cache with writable bind' "$writable_natives" no
for database in agent.db models.db skill-descriptions.db; do
  for suffix in '' -wal -shm; do
    assert "Linux allows runtime database $database$suffix" has_linux_bind --bind "$linux_agent/$database$suffix"
  done
done
for path in agent/sessions logs cache run; do
  assert "Linux permits runtime subtree $path" has_linux_bind --bind "$linux_policy/home/custom/profiles/glen-harness/$path"
done
assert 'Linux file preparation preserves existing database contents' cmp -s "$linux_agent/agent.db" "$linux_policy/database-before"

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

# Fix cycle 5, decision 3/8: the engine refuses Linux unconditionally, before
# ever checking for bwrap, so the refusal must appear with AND without bwrap
# on PATH. This block runs on every host (not gated to non-Darwin) because
# the refusal is itself platform-independent code, testable by temporarily
# reporting a fake uname.
linux_probe_bin="$WORK/linux-probe-bin"
mkdir -p "$linux_probe_bin" "$WORK/linux-probe-home"
cat > "$linux_probe_bin/uname" <<'EOF'
#!/bin/sh
printf 'Linux
'
EOF
chmod +x "$linux_probe_bin/uname"
for tool in date git dirname jq omp; do
  ln -s "$(command -v "$tool")" "$linux_probe_bin/$tool"
done
run env HOME="$WORK/linux-probe-home" PI_CONFIG_DIR=.omp GLEN_OMP_ENGINE=on \
  PATH="$linux_probe_bin" /bin/bash "$ENGINE" probe
assert_exit 'Linux probe without bwrap refuses before any tool check' 3
assert 'Linux probe without bwrap has skipped status' jq -e '.status == "skipped"' "$STDOUT"
assert_contains 'Linux probe without bwrap names the unavailable reason' \
  'unavailable: linux abstract-socket isolation not supported' "$STDOUT"

cp "$WORK/bin/glen" "$linux_probe_bin/bwrap"
chmod +x "$linux_probe_bin/bwrap"
run env HOME="$WORK/linux-probe-home" PI_CONFIG_DIR=.omp GLEN_OMP_ENGINE=on \
  PATH="$linux_probe_bin" /bin/bash "$ENGINE" probe
assert_exit 'Linux probe with bwrap present still refuses' 3
assert_contains 'Linux probe with bwrap present names the same reason' \
  'unavailable: linux abstract-socket isolation not supported' "$STDOUT"
assert 'Linux probe invokes neither omp nor the fake bwrap' test ! -e "$SMOKE_UNEXPECTED_CALL"

# Hide the sandbox tool with an allowlisted PATH: prepending an empty directory
# cannot hide a later executable. Both the probe and bypassed-probe dispatch fail.
mkdir "$WORK/no-sandbox-bin"
for tool in date uname git dirname jq omp mktemp rm; do
  ln -s "$(command -v "$tool")" "$WORK/no-sandbox-bin/$tool"
done
# Fix cycle 5, decision 8: on a real (not faked) Linux host, probe_checks and
# resolve_launcher both refuse before ever reaching a bwrap-on-PATH check, so
# the expected reason differs by host platform, not by this fixture.
if [ "$(uname -s)" = Darwin ]; then
  no_sandbox_reason='sandbox-exec not on PATH'
else
  no_sandbox_reason='unavailable: linux abstract-socket isolation not supported'
fi
run env HOME="$WORK/home" PI_CONFIG_DIR=.omp GLEN_OMP_ENGINE=on \
  PATH="$WORK/no-sandbox-bin" /bin/bash "$ENGINE" probe
assert_exit 'probe without sandbox skips' 3
assert 'probe without sandbox has skipped status' jq -e '.status == "skipped"' "$STDOUT"
assert_contains 'probe without sandbox names the reason' "$no_sandbox_reason" "$STDOUT"
assert 'probe without sandbox creates no run root or jobs' test ! -e "$WORK/tmp/glen-omp-engine"
run env HOME="$WORK/home" PATH="$WORK/no-sandbox-bin" /bin/bash "$ENGINE" envcheck -- "$GLEN_BIN"
assert_exit 'dispatch without sandbox fails closed' 4
assert_contains 'dispatch without sandbox names the reason' "$no_sandbox_reason" "$STDERR"
assert 'missing sandbox never executes omp or glen' test ! -e "$SMOKE_UNEXPECTED_CALL"

# Offline routing checks exercise the REAL launcher with a fake OS sandbox tool,
# independently of whether nested sandbox_apply is permitted by the test host.
export SMOKE_SANDBOX_CALLS="$WORK/sandbox-calls.jsonl"
cat > "$WORK/bin/sandbox-exec" <<'EOF'
#!/bin/bash
set -eu
[ "$1" = -f ] && [ -f "$2" ] || exit 99
shift 2
export SMOKE_SANDBOX_ACTIVE=1
jq -cn --arg root "$OMP_SANDBOX_RUN_ROOT" --arg cwd "$(pwd -P)" \
  --arg agent "$OMP_PROFILE_AGENT_DIR" --arg binary "$OMP_REAL_BIN" \
  '{root:$root,cwd:$cwd,agent:$agent,binary:$binary}' >> "$SMOKE_SANDBOX_CALLS"
exec "$@"
EOF
cat > "$WORK/bin/bwrap" <<'EOF'
#!/bin/bash
set -eu
while [ "$1" != -- ]; do shift; done
shift
export SMOKE_SANDBOX_ACTIVE=1
jq -cn --arg root "$OMP_SANDBOX_RUN_ROOT" --arg cwd "$(pwd -P)" \
  --arg agent "$OMP_PROFILE_AGENT_DIR" --arg binary "$OMP_REAL_BIN" \
  '{root:$root,cwd:$cwd,agent:$agent,binary:$binary}' >> "$SMOKE_SANDBOX_CALLS"
exec "$@"
EOF
chmod +x "$WORK/bin/sandbox-exec" "$WORK/bin/bwrap"

started="$(now_ms)"
run env GLEN_OMP_ENGINE=off /bin/bash "$ENGINE" probe
elapsed=$(( $(now_ms) - started ))
assert_exit 'probe disabled exit' 3
assert 'probe disabled status' jq -e '.status == "skipped"' "$STDOUT"
assert_contains 'probe disabled reason' GLEN_OMP_ENGINE "$STDOUT"
assert "probe disabled takes under two seconds (${elapsed}ms)" test "$elapsed" -lt 2000

# Codex P2 / Claude LOW-1: on real Linux, probe_checks refuses before any
# profile or run-root check, so those reasons are macOS-only expectations.
if [ "$(uname -s)" = Darwin ]; then
  missing_profile_reason='profile is not set up'
  # shellcheck disable=SC2016 # The engine's diagnostic deliberately names HOME literally.
  home_root_reason='run root under $HOME'
else
  missing_profile_reason='unavailable: linux abstract-socket isolation not supported'
  home_root_reason="$missing_profile_reason"
fi
run env HOME="$WORK/home" PI_CONFIG_DIR=.omp GLEN_OMP_ENGINE=on /bin/bash "$ENGINE" probe
assert_exit 'probe missing profile exit' 3
assert_contains 'probe missing profile reason' "$missing_profile_reason" "$STDOUT"
assert 'probe missing profile creates no run root or jobs' test ! -e "$WORK/tmp/glen-omp-engine"

mkdir -p "$WORK/home/.omp/profiles/glen-harness/agent" "$WORK/home/.omp/natives" "$WORK/home/tmproot"
printf 'tools:\n  approvalMode: always-ask\n' > "$WORK/home/.omp/profiles/glen-harness/agent/config.yml"
run env HOME="$WORK/home" PI_CONFIG_DIR=.omp TMPDIR="$WORK/home/tmproot" GLEN_OMP_ENGINE=on /bin/bash "$ENGINE" probe
assert_exit 'probe run root under home exit' 3
assert_contains 'probe run root under home reason' "$home_root_reason" "$STDOUT"
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
mkdir -p "$SRC/nested/.github" "$SRC/.github" "$SRC/.wrangler/state" "$SRC/nested/.wrangler/state" "$SRC/mixed-case/.github"
for path in .env .env.local .env.production .env.production.local nested/.env.development.local .claude/.env mixed-case/.ENV mixed-case/.Env.Local; do
  printf 'CLAUDE_CONFIG_DIR=_agent-config/claude\n' > "$SRC/$path"
done
printf 'local database\n' > "$SRC/.wrangler/state/db"
printf 'nested local database\n' > "$SRC/nested/.wrangler/state/db"
printf '@~/.ssh/x\nSee @./local\\ path and\t@../../outside.\nKeep user@example.com readable.\n' >> "$SRC/AGENTS.md"
printf 'See @/outside and @relative\n' > "$SRC/nested/AGENTS.md"
printf 'See @~/private\n' > "$SRC/nested/CLAUDE.md"
printf 'See @../private\n' > "$SRC/.github/copilot-instructions.md"
printf 'See @~/private\n' > "$SRC/nested/.github/copilot-instructions.md"
for path in agents.md Claude.MD .github/COPILOT-INSTRUCTIONS.MD WATCHDOG.md WATCHDOG.yml; do
  printf 'See @~/private\n' > "$SRC/mixed-case/$path"
  assert "mixed-case/watchdog fixture $path has an import" has_at_imports "$SRC/mixed-case/$path"
done
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
for path in .env .env.local .env.production .env.production.local nested/.env.development.local .claude/.env mixed-case/.ENV mixed-case/.Env.Local; do
  assert "snapshot removes dotenv $path" test ! -e "$DEST/$path"
  assert_contains "snapshot preserves dotenv $path" 'CLAUDE_CONFIG_DIR=_agent-config/claude' "$DEST/_agent-config/dotenv/$path"
done
assert_eq 'snapshot has no active dotenv files' "$(find "$DEST" -path "$DEST/_agent-config/dotenv" -prune -o \( -iname .env -o -iname '.env.*' \) -print)" ''
assert 'snapshot excludes unignored root wrangler state' test ! -e "$DEST/.wrangler"
assert 'snapshot excludes unignored nested wrangler state' test ! -e "$DEST/nested/.wrangler"
assert 'snapshot keeps ordinary untracked content beside state' test -f "$DEST/untracked.txt"
for path in AGENTS.md CLAUDE.md nested/AGENTS.md nested/CLAUDE.md .github/copilot-instructions.md nested/.github/copilot-instructions.md context-dir/AGENTS.md \
  mixed-case/agents.md mixed-case/Claude.MD mixed-case/.github/COPILOT-INSTRUCTIONS.MD mixed-case/WATCHDOG.md mixed-case/WATCHDOG.yml; do
  assert "snapshot preserves context file $path" test -f "$DEST/$path"
  assert "snapshot neutralizes imports in $path" no_at_imports "$DEST/$path"
done
assert_contains 'snapshot keeps import path readable' '@~/.ssh/x' "$DEST/AGENTS.md"
assert_contains 'snapshot keeps email unchanged' 'user@example.com' "$DEST/AGENTS.md"
assert_eq 'snapshot leaves harmless context unchanged' "$(head -n 1 "$DEST/AGENTS.md")" '# Fixture conventions'
assert 'snapshot accepts internal file symlink' test -L "$DEST/CLAUDE.md"
assert 'snapshot accepts internal directory symlink' test -L "$DEST/context-dir"

# Ancestors above the engine run root are never copied/sanitized into the tree.
for name in AGENTS.md cLaUdE.mD; do
  ancestor="$WORK/ancestor-context-$name"
  mkdir -p "$ancestor/glen-omp-engine/run"
  printf 'Hostile ancestor context\n' > "$ancestor/$name"
  run /bin/bash "$ENGINE" snapshot "$SRC" "$ancestor/glen-omp-engine/run/tree"
  assert_exit "snapshot refuses ancestor context $name" 3
  assert "ancestor context $name is a skip" jq -e '.status == "skipped"' "$STDOUT"
  # Fix cycle 5, decision 6: the bracket-class glob is a real metacharacter
  # pattern, so it now returns the exact on-disk name (its own case variant),
  # not the pattern's own casing; check the full exact path.
  assert_contains "ancestor context $name reason names file" "$ancestor/$name" "$STDOUT"
  assert_contains "ancestor context $name reason names problem" 'context file' "$STDOUT"
done
ancestor="$WORK/ancestor-permissions"
mkdir -p "$ancestor/glen-omp-engine/run"
chmod 0770 "$ancestor"
run /bin/bash "$ENGINE" snapshot "$SRC" "$ancestor/glen-omp-engine/run/tree"
assert_exit 'snapshot refuses group-writable ancestor above run root' 3
assert 'writable ancestor is a skip' jq -e '.status == "skipped"' "$STDOUT"
assert_contains 'writable ancestor reason names directory' "$ancestor" "$STDOUT"
assert_contains 'writable ancestor reason names mode problem' 'group- or world-writable' "$STDOUT"
chmod 0700 "$ancestor"
run /bin/bash "$ENGINE" snapshot "$SRC" "$ancestor/glen-omp-engine/run/clean-tree"
assert_exit 'snapshot clean ancestor chain succeeds' 0
assert 'snapshot clean ancestor chain status' jq -e '.status == "ok"' "$STDOUT"
assert 'snapshot clean ancestor chain copied content' test -f "$ancestor/glen-omp-engine/run/clean-tree/tracked file.txt"

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
assert_exit 'snapshot refuses tracked external context symlink' 3
assert 'snapshot external context symlink is a skip' jq -e '.status == "skipped"' "$STDOUT"
assert_contains 'snapshot external context symlink reason' 'unsafe symlink' "$STDOUT"
assert_contains 'snapshot external context symlink diagnostic names link' "$WORK/link-tracked/AGENTS.md" "$STDERR"
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
  assert_exit "snapshot refuses untracked $kind symlink" 3
  assert "snapshot $kind symlink is a skip" jq -e '.status == "skipped"' "$STDOUT"
  assert_contains "snapshot $kind symlink reason" 'unsafe symlink' "$STDOUT"
  assert_contains "snapshot $kind symlink diagnostic names location" "$WORK/link-$kind/" "$STDERR"
  rm "$LINK_SRC/link"
  [ "$kind" != chain ] || rm "$LINK_SRC/first"
done
mkdir -p "$LINK_SRC/.claude"
ln -s ../plain "$LINK_SRC/.claude/link"
run /bin/bash "$ENGINE" snapshot "$LINK_SRC" "$WORK/link-relocation"
assert_exit 'snapshot revalidates relative links after relocation' 3
assert 'snapshot relocated symlink is a skip' jq -e '.status == "skipped"' "$STDOUT"
assert_contains 'snapshot relocation symlink reason' 'unsafe symlink' "$STDOUT"
assert_contains 'snapshot relocation symlink diagnostic names link' "$WORK/link-relocation/_agent-config/claude/link" "$STDERR"
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
# Fix cycle 6, Codex P2: an edit to an existing untracked file mid-capture
# changes the fingerprint too, so the snapshot never keeps stale bytes.
# The marker lives outside SRC: a new untracked name would change status by itself.
# shellcheck disable=SC2016 # Expanded by the snapshot subprocess, not this shell.
run env SMOKE_RACE_MARKER="$WORK/race-untracked-marker" \
  GLEN_OMP_ENGINE_TEST_MUTATE='if [ ! -e "$SMOKE_RACE_MARKER" ]; then printf "untracked race\n" >> untracked.txt; : > "$SMOKE_RACE_MARKER"; fi' \
  /bin/bash "$ENGINE" snapshot "$SRC" "$WORK/race-untracked"
assert_exit 'capture untracked race recaptures successfully' 0
assert_contains 'capture untracked race retains new untracked content' 'untracked race' "$WORK/race-untracked/untracked.txt"
assert_contains 'capture untracked race logs the recapture' 'source changed during capture; recapturing once' "$STDERR"
# shellcheck disable=SC2016 # Expanded by the snapshot subprocess, not this shell.
run env SMOKE_MUTATION_MARKER="$WORK/direct-mutation" GLEN_OMP_ENGINE_TEST_MUTATE=': > "$SMOKE_MUTATION_MARKER"' \
  /bin/bash "$ENGINE" snapshot "$SRC" "$WORK/direct-mutation-snapshot"
assert_exit 'standalone snapshot explicit mutation hook exit' 0
assert 'standalone snapshot honors explicit mutation hook' test -f "$WORK/direct-mutation"

POLICY_REPO="$WORK/policy-source"
mkdir -p "$POLICY_REPO/.claude/omp"
git -C "$POLICY_REPO" init -q --initial-branch=main
cp "$SANDBOX" "$LINUX_SANDBOX" "$POLICY_REPO/.claude/omp/"
printf '# main policy\ntools:\n  approvalMode: always-ask\n' > "$POLICY_REPO/.claude/omp/glen-harness.yml"
git -C "$POLICY_REPO" add .
commit_fixture "$POLICY_REPO"
printf '# worktree policy\ntools:\n  approvalMode: yolo\n' > "$POLICY_REPO/.claude/omp/glen-harness.yml"
run /bin/bash "$ENGINE" policy "$POLICY_REPO"
assert_exit 'policy exit' 0
assert_eq 'policy source label' "$(sed -n '1p' "$STDOUT")" main
assert_eq 'policy content comes from main' "$(sed '1d' "$STDOUT")" \
  "$(git -C "$POLICY_REPO" show main:.claude/omp/glen-harness.yml)"

# Fix cycle 6 (Claude LOW-1 / Codex P2): launcher resolution, the
# environment boundary and every probe/dispatch below need macOS
# sandbox-exec; resolve_launcher and probe_checks refuse real Linux outright.
if [ "$(uname -s)" != Darwin ]; then
  mkdir "$WORK/launcher-linux"
  run /bin/bash "$ENGINE" launcher "$POLICY_REPO" "$WORK/launcher-linux"
  assert_exit 'Linux launcher resolution refuses' 4
  assert_contains 'Linux launcher refusal names the reason' 'unavailable: linux abstract-socket isolation not supported' "$STDERR"
  assert 'Linux launcher refusal returns no launcher path' test ! -s "$STDOUT"
  run env HOME="$WORK/home" PI_CONFIG_DIR=.omp GLEN_OMP_ENGINE=on /bin/bash "$ENGINE" probe
  assert_exit 'Linux probe with a set-up profile still refuses' 3
  assert_contains 'Linux probe with a set-up profile names the reason' 'unavailable: linux abstract-socket isolation not supported' "$STDOUT"
  assert 'Linux probe never calls glen or omp' test ! -e "$SMOKE_UNEXPECTED_CALL"
  printf 'NOTE launcher, probe and dispatch suites skipped: Darwin with sandbox-exec required\n'
else
launcher_name=omp-sandboxed.sh
printf '#!/bin/bash\nexit 99\n' > "$POLICY_REPO/.claude/omp/$launcher_name"
mkdir "$WORK/launcher-main" "$WORK/launcher-fallback"
run /bin/bash "$ENGINE" launcher "$POLICY_REPO" "$WORK/launcher-main"
assert_exit 'launcher resolution succeeds from main' 0
captured_launcher="$(cat "$STDOUT")"
git -C "$POLICY_REPO" show "main:.claude/omp/$launcher_name" > "$WORK/main-launcher"
assert 'launcher uses main despite modified worktree launcher' cmp -s "$WORK/main-launcher" "$captured_launcher"
assert 'captured launcher is a regular executable file' test -f "$captured_launcher" -a -x "$captured_launcher"
assert 'main launcher resolution needs no warning' test ! -s "$STDERR"
FALLBACK_REPO="$WORK/launcher-source-without-main"
mkdir -p "$FALLBACK_REPO/.claude/omp"
git -C "$FALLBACK_REPO" init -q --initial-branch=fixture
cp "$SANDBOX" "$LINUX_SANDBOX" "$FALLBACK_REPO/.claude/omp/"
run /bin/bash "$ENGINE" launcher "$FALLBACK_REPO" "$WORK/launcher-fallback"
assert_exit 'launcher without main uses worktree fallback' 0
assert 'launcher fallback captures worktree contents' cmp -s "$FALLBACK_REPO/.claude/omp/$launcher_name" "$(cat "$STDOUT")"
assert_contains 'launcher fallback warns about source' 'warning: main launcher unavailable; using the worktree launcher' "$STDERR"
mkdir "$WORK/launcher-missing" "$WORK/no-launcher-source"
run /bin/bash "$ENGINE" launcher "$WORK/no-launcher-source" "$WORK/launcher-missing"
assert_exit 'launcher missing from both sources fails closed' 4
assert 'missing launcher never returns an executable path' test ! -s "$STDOUT"

# Fix cycle 5, decision 1: physical containment. A destination that IS a
# per-dispatch run directory is refused lexically; one reached only through a
# symlink into such a directory is refused once pwd -P resolves it.
mkdir -p "$WORK/launcher-run-dirs/glen-omp-engine/run-fake-20260101T000000Z-1"
run /bin/bash "$ENGINE" launcher "$POLICY_REPO" "$WORK/launcher-run-dirs/glen-omp-engine/run-fake-20260101T000000Z-1"
assert_exit 'launcher placed in a run directory is refused' 4
assert_contains 'run directory refusal names the reason' 'per-dispatch run directory' "$STDERR"
assert 'run directory refusal returns no launcher path' test ! -s "$STDOUT"

mkdir -p "$WORK/launcher-run-target/glen-omp-engine/run-fake-20260101T000000Z-2"
ln -s "$WORK/launcher-run-target/glen-omp-engine/run-fake-20260101T000000Z-2" "$WORK/launcher-run-symlink"
run /bin/bash "$ENGINE" launcher "$POLICY_REPO" "$WORK/launcher-run-symlink"
assert_exit 'launcher dir symlinked into a run directory is refused' 4
assert_contains 'symlinked run directory refusal names the reason' 'per-dispatch run directory' "$STDERR"
assert 'symlinked run directory refusal returns no launcher path' test ! -s "$STDOUT"

# Fix cycle 6 (Claude MED-1 / Codex P1): every sandbox-writable root counts
# after resolution, not only run-* names: profile siblings, shared ~/.omp/run,
# an engine probe root, and an active run root of any name, also via symlink.
containment_home="$WORK/containment-home"
mkdir -p "$containment_home/.omp/profiles/glen-harness/logs" "$containment_home/.omp/profiles/glen-harness/cache" \
  "$containment_home/.omp/run" "$WORK/containment-tmp/glen-omp-engine/probe.fixture" "$WORK/containment-active/omp.fixture"
ln -s "$containment_home/.omp/profiles/glen-harness/logs" "$WORK/containment-logs-link"
ln -s "$WORK/containment-active/omp.fixture" "$WORK/containment-active-link"
for containment_case in "profile-logs $containment_home/.omp/profiles/glen-harness/logs" \
  "profile-cache $containment_home/.omp/profiles/glen-harness/cache" "shared-run $containment_home/.omp/run" \
  "logs-symlink $WORK/containment-logs-link" "probe-root $WORK/containment-tmp/glen-omp-engine/probe.fixture" \
  "active-root-symlink $WORK/containment-active-link"; do
  label="${containment_case%% *}" target="${containment_case#* }"
  run env HOME="$containment_home" OMP_SANDBOX_RUN_ROOT="$WORK/containment-active/omp.fixture" \
    /bin/bash "$ENGINE" launcher "$POLICY_REPO" "$target"
  assert_exit "launcher refuses a $label destination" 4
  assert_contains "launcher $label refusal names a writable root" 'sandbox launcher must not resolve' "$STDERR"
  assert "launcher $label refusal returns no launcher path" test ! -s "$STDOUT"
done
run env HOME="$containment_home" TMPDIR="$containment_home/.omp/profiles/glen-harness/cache" \
  /bin/bash "$ENGINE" launcher "$POLICY_REPO"
assert_exit 'launcher refuses a default cache under a TMPDIR inside the profile' 4

# A cached launcher is reused only as a regular file with the same bytes.
mkdir "$WORK/launcher-reuse"
run /bin/bash "$ENGINE" launcher "$POLICY_REPO" "$WORK/launcher-reuse"
reuse_launcher="$(cat "$STDOUT")"
chmod 0700 "$reuse_launcher"
printf '#!/bin/bash\necho tampered\n' > "$reuse_launcher"
chmod 0500 "$reuse_launcher"
run /bin/bash "$ENGINE" launcher "$POLICY_REPO" "$WORK/launcher-reuse"
assert_exit 'launcher replaces a tampered cached file' 0
assert 'replaced launcher has the main bytes again' cmp -s "$WORK/main-launcher" "$reuse_launcher"
rm -f "$reuse_launcher"
ln -s "$WORK/main-launcher" "$reuse_launcher"
run /bin/bash "$ENGINE" launcher "$POLICY_REPO" "$WORK/launcher-reuse"
assert_exit 'launcher replaces a symlink at the cached name' 0
assert 'replaced launcher is a regular file, not a symlink' test -f "$reuse_launcher" -a ! -L "$reuse_launcher"
# Claude LOW-4: a fresh sibling (another worktree's version) survives;
# only a day-old one is pruned.
printf 'other version\n' > "$WORK/launcher-reuse/omp-sandboxed.sh.0000000000000000"
printf 'old version\n' > "$WORK/launcher-reuse/omp-sandboxed.sh.1111111111111111"
printf 'abandoned fetch\n' > "$WORK/launcher-reuse/.omp-sandboxed.sh.fetch.old"
touch -t 202001010000 "$WORK/launcher-reuse/omp-sandboxed.sh.1111111111111111" "$WORK/launcher-reuse/.omp-sandboxed.sh.fetch.old"
run /bin/bash "$ENGINE" launcher "$POLICY_REPO" "$WORK/launcher-reuse"
assert_exit 'launcher resolution with stale siblings succeeds' 0
assert 'fresh sibling launcher is kept for a concurrent engine' test -f "$WORK/launcher-reuse/omp-sandboxed.sh.0000000000000000"
assert 'day-old sibling launcher is pruned' test ! -e "$WORK/launcher-reuse/omp-sandboxed.sh.1111111111111111"
assert 'abandoned fetch file is pruned' test ! -e "$WORK/launcher-reuse/.omp-sandboxed.sh.fetch.old"

# Use a synthetic token. Never print envcheck output or any variable values.
(
  export CLAUDE_CONFIG_DIR=smoke PI_PY=smoke PI_JS=smoke PI_CODING_AGENT_DIR=smoke
  export COPILOT_CUSTOM_INSTRUCTIONS_DIRS=smoke
  export OMP_CODING_AGENT_DIR=smoke OMP_PROFILE=smoke PI_PROFILE=smoke
  export GLEN_OMP_BIN=hostile-inherited OMP_SANDBOX_RUN_ROOT=hostile-inherited
  export OMP_PROFILE_AGENT_DIR=hostile-inherited OMP_REAL_BIN=hostile-inherited
  export MNEMOSYNE_TOKEN=smoke-placeholder
  HOME="$WORK/home" /bin/bash "$ENGINE" envcheck -- /bin/sh -c 'test -f "$GLEN_OMP_BIN" && test -x "$GLEN_OMP_BIN" && env'
) > "$WORK/env.out" 2> "$STDERR"
for name in CLAUDE_CONFIG_DIR COPILOT_CUSTOM_INSTRUCTIONS_DIRS PI_PY PI_JS PI_CODING_AGENT_DIR OMP_CODING_AGENT_DIR OMP_PROFILE PI_PROFILE; do
  if grep -q "^${name}=" "$WORK/env.out"; then present=yes; else present=no; fi
  assert_eq "dispatch scrubs $name" "$present" no
done
assert 'dispatch retains token variable name' grep -q '^MNEMOSYNE_TOKEN=' "$WORK/env.out"
assert 'dispatch selects captured platform launcher' grep -Eq "^GLEN_OMP_BIN=.*/$launcher_name\.[^/]+$" "$WORK/env.out"
assert_contains 'dispatch passes explicit profile agent path' "OMP_PROFILE_AGENT_DIR=$WORK/home/.omp/profiles/glen-harness/agent" "$WORK/env.out"
assert_contains 'dispatch overwrites inherited real binary' "OMP_REAL_BIN=$WORK/bin/omp" "$WORK/env.out"

# Successful production paths use offline stubs, never the owner's omp/profile.
export SMOKE_OMP_CALLS="$WORK/omp-calls" SMOKE_DISPATCH_CALLS="$WORK/dispatch-calls"
export SMOKE_OMP_FDS="$WORK/omp-fds" SMOKE_DISPATCH_FDS="$WORK/dispatch-fds"
export SMOKE_SESSION="$WORK/session.jsonl" SMOKE_SEND_ARGV="$WORK/send-argv"
: > "$SMOKE_SEND_ARGV"
printf '%s\n' '{"type":"message","message":{"role":"assistant","model":"fixture","provider":"cursor","usage":{"input":1,"output":1}}}' > "$SMOKE_SESSION"
cat > "$WORK/bin/omp" <<'EOF'
#!/bin/sh
# The launcher's unsandboxed daemon-scope hash (Bun runtime mode).
if [ "${BUN_BE_BUN:-}" = 1 ]; then printf '0123456789abcdef'; exit 0; fi
[ "${SMOKE_SANDBOX_ACTIVE:-}" = 1 ] || exit 98
if [ -e /dev/fd/3 ]; then printf 'open\n' >> "$SMOKE_OMP_FDS"; exit 99; fi
printf 'closed\n' >> "$SMOKE_OMP_FDS"
printf '%s\n' "$*" >> "$SMOKE_OMP_CALLS"
case "$*" in
  '--profile glen-harness models cursor --json') printf '%s\n' '{"models":[{"id":"fixture"}]}' ;;
  '--profile glen-harness config get modelRoles --json')
    [ "${SMOKE_REJECT_MODEL_ROLES:-0}" = 0 ] || exit 99
    printf '%s\n' "{\"value\":{\"default\":\"${SMOKE_MODEL_ROLES_DEFAULT:-cursor/fixture}\",\"task\":\"cursor/other\"}}"
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
[ "${COPILOT_CUSTOM_INSTRUCTIONS_DIRS+x}" != x ] || exit 99
fd3=closed
[ ! -e /dev/fd/3 ] || fd3=open
printf '%s %s\n' "$command" "$fd3" >> "$SMOKE_DISPATCH_FDS"
[ "$fd3" = closed ] || exit 99
[ -f "${GLEN_OMP_BIN:-}" ] && [ -x "$GLEN_OMP_BIN" ] || exit 99
# Fix cycle 4, item 1: the launcher must never sit inside the per-dispatch run
# root (OMP_SANDBOX_RUN_ROOT), which the sandboxed child can write to. It now
# lives in a fixed, per-TMPDIR cache dir outside any such root.
case "$GLEN_OMP_BIN" in "$TMPDIR"/glen-omp-engine-launcher/omp-sandboxed*.sh.*) ;; *) exit 99 ;; esac
case "$GLEN_OMP_BIN" in "$OMP_SANDBOX_RUN_ROOT"/*) exit 99 ;; esac
[ "$OMP_PROFILE_AGENT_DIR" = "$HOME/.omp/profiles/glen-harness/agent" ] || exit 99
[ -d "${OMP_SANDBOX_RUN_ROOT:-}/tree" ] || exit 99
printf '%s scrubbed\n' "$command" >> "$SMOKE_DISPATCH_CALLS"
case "$command" in
  send)
    printf '%s\n' "$*" >> "${SMOKE_SEND_ARGV:-/dev/null}"
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
assert 'fd3 fixture detects an intentionally open descriptor' /bin/bash -c 'test -e /dev/fd/3' 3> "$WORK/fd3-control"
assert_eq 'probe omp children start with fd3 closed' "$(cat "$SMOKE_OMP_FDS")" $'closed\nclosed'
assert_contains 'probe queries registered modelRoles record key' 'config get modelRoles --json' "$SMOKE_OMP_CALLS"
assert_eq 'probe never queries invalid modelRoles.default key' "$(grep -c 'config get modelRoles.default' "$SMOKE_OMP_CALLS" || true)" 0
# shellcheck disable=SC2016 # jq receives these variables through --arg.
assert 'probe routes both calls through launcher with private cwd and explicit paths' jq -es \
  --arg agent "$WORK/home/.omp/profiles/glen-harness/agent" --arg binary "$WORK/bin/omp" \
  'length==2 and all(.[]; .root==.cwd and .agent==$agent and .binary==$binary)' "$SMOKE_SANDBOX_CALLS"
assert 'probe reports launcher source' jq -e '.launcher_source == "main" or .launcher_source == "worktree"' "$STDOUT"

# Exercise both bounded_omp branches, independent of which timeout tools the
# host installed. The private PATH still points at the same fd-aware fake omp.
mkdir "$WORK/no-timeout-bin" "$WORK/with-timeout-bin"
for tool in date dirname git jq mkdir stat id mktemp perl rm uname find env omp cp chmod cat readlink sandbox-exec shasum sha256sum awk cmp touch tr mv; do
  if command -v "$tool" >/dev/null 2>&1; then ln -s "$(command -v "$tool")" "$WORK/no-timeout-bin/$tool"; fi
done
cat > "$WORK/with-timeout-bin/timeout" <<'EOF'
#!/bin/sh
: > "$SMOKE_TIMEOUT_USED"
shift
exec "$@"
EOF
chmod +x "$WORK/with-timeout-bin/timeout"
export SMOKE_TIMEOUT_USED="$WORK/timeout-used"
for branch in timeout perl; do
  probe_path="$WORK/no-timeout-bin"
  [ "$branch" != timeout ] || probe_path="$WORK/with-timeout-bin:$probe_path"
  : > "$SMOKE_OMP_FDS"
  run env HOME="$WORK/home" PI_CONFIG_DIR=.omp GLEN_OMP_ENGINE=on \
    PATH="$probe_path" /bin/bash "$ENGINE" probe
  assert_exit "probe $branch fd3 check succeeds" 0
  assert_eq "probe $branch closes fd3 for both omp calls" "$(cat "$SMOKE_OMP_FDS")" $'closed\nclosed'
  if [ "$branch" = timeout ]; then
    assert 'probe timeout fixture actually ran' test -e "$SMOKE_TIMEOUT_USED"
    rm "$SMOKE_TIMEOUT_USED"
  else
    assert 'probe perl branch does not run timeout' test ! -e "$SMOKE_TIMEOUT_USED"
  fi
done

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
assert_eq 'send children have fd3 closed on every call' "$(grep -c '^send closed$' "$SMOKE_DISPATCH_FDS")" 4
assert_eq 'wait children have fd3 closed on every call' "$(grep -c '^wait closed$' "$SMOKE_DISPATCH_FDS")" 4
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

# Fix cycle 5, decision 8: this whole section exercises macOS's
# sandbox-exec-specific dispatch path (resolve_launcher/probe_checks both
# refuse Linux unconditionally), so it stays inside the Darwin gate above.
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
    # Items 11 and 7 (fix cycle 4/5): the fixture model (cursor/fixture) is
    # off-ladder (an explicit --model stand-in), so no --thinking at all
    # reaches the real send argv ($STATE/calls.jsonl, written by
    # tests/support/glen's own argv capture, not $WORK/bin/glen's).
    assert 'off-ladder model sends no --thinking at all' jq -es \
      'all(.[]; (.argv | index("--thinking")) == null)' "$STATE/calls.jsonl"
  fi
done

# Items 11 and 7, fix cycle 6 MED-2: --thinking repeats exactly the persisted
# suffix, including the grok rung's "high" and a suffix that differs from
# the ladder, and is omitted for a bare id even when that id is on the ladder
# (setup --model cursor/grok-4.7 persists no suffix).
for ladder_case in argon grok override bare; do
  case "$ladder_case" in
    argon) ladder_model=cursor/gemini-4-argon:medium expected_level=medium ;;
    grok) ladder_model=cursor/grok-4.7:high expected_level=high ;;
    override) ladder_model=cursor/gemini-4-argon:low expected_level=low ;;
    bare) ladder_model=cursor/grok-4.7 expected_level='' ;;
  esac
  dispatch_fixture "ladder-$ladder_case"
  dispatch_step send 0 "$DONE"
  SMOKE_MODEL_ROLES_DEFAULT="$ladder_model" dispatch_run
  assert_exit "model role $ladder_model dispatch succeeds" 0
  if [ -n "$expected_level" ]; then
    # shellcheck disable=SC2016 # jq binds $level through --arg.
    assert "model role $ladder_model sends its persisted thinking level" jq -es \
      --arg level "$expected_level" \
      'length > 0 and all(.[]; (.argv | index("--thinking")) as $i | $i != null and .argv[$i+1] == $level)' \
      "$STATE/calls.jsonl"
  else
    assert "model role $ladder_model sends no --thinking at all" jq -es \
      'length > 0 and all(.[]; (.argv | index("--thinking")) == null)' "$STATE/calls.jsonl"
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
fi
printf '%s assertions, %s failures\n' "$CASES" "$FAILURES"
[ "$FAILURES" -eq 0 ]
