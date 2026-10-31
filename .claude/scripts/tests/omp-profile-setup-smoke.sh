#!/usr/bin/env bash
# Offline W9 setup smoke: fresh HOME/TMPDIR per invocation, fake omp on PATH.
set -euo pipefail
umask 077

TEST_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$TEST_DIR/../../.." && pwd -P)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/glen-omp-setup-smoke.XXXXXX")"
WORK="$(cd "$WORK" && pwd -P)"
trap 'rm -rf "$WORK"' EXIT
# A private engine checkout supplies a known main launcher. This both isolates
# source selection from the host's main branch and exercises the shared resolver.
ENGINE_FIXTURE="$WORK/engine-source"
mkdir -p "$ENGINE_FIXTURE/.claude/scripts" "$ENGINE_FIXTURE/.claude/omp/skills/mnemosyne-glen"
cp "$REPO_ROOT/.claude/scripts/omp-profile-setup.sh" "$REPO_ROOT/.claude/scripts/omp-engine.sh" "$ENGINE_FIXTURE/.claude/scripts/"
cp "$REPO_ROOT/.claude/omp/omp-sandboxed.sh" "$REPO_ROOT/.claude/omp/omp-sandboxed-linux.sh" \
  "$REPO_ROOT/.claude/omp/glen-harness.yml" "$REPO_ROOT/.claude/omp/mcp.json" "$ENGINE_FIXTURE/.claude/omp/"
cp "$REPO_ROOT/.claude/omp/skills/mnemosyne-glen/SKILL.md" "$ENGINE_FIXTURE/.claude/omp/skills/mnemosyne-glen/"
git -C "$ENGINE_FIXTURE" init -q --initial-branch=main
git -C "$ENGINE_FIXTURE" add .
git -C "$ENGINE_FIXTURE" -c user.name='Glen smoke' -c user.email='smoke@example.invalid' \
  -c core.hooksPath=/dev/null -c commit.gpgsign=false commit --no-gpg-sign -qm fixture
SETUP="$ENGINE_FIXTURE/.claude/scripts/omp-profile-setup.sh"
mkdir "$WORK/bin"
cp "$TEST_DIR/support/omp" "$WORK/bin/omp-base"
export SMOKE_OMP_BASE="$WORK/bin/omp-base"
# Simulate native extraction only in the unsandboxed tokenizer path. Models
# refuses a cold cache, so routing through the launcher alone cannot satisfy it.
cat > "$WORK/bin/omp" <<'EOF'
#!/bin/bash
set -eu
if [ "$*" = '--profile glen-harness toks glen-native-warmup --json' ]; then
  [ "$HOME" = "$FAKE_OMP_TEST_HOME" ] || exit 99
  [ "${FAKE_OMP_SANDBOX_ACTIVE:-0}" = 0 ] || exit 98
  [ "${FAKE_OMP_PREWARM_FAIL:-0}" = 0 ] || exit 97
  mkdir -p "$HOME/.omp/natives"
  : > "$HOME/.omp/natives/prewarmed"
  jq -cn --arg home "$HOME" --args '{home:$home,argv:$ARGS.positional}' -- "$@" >> "$FAKE_OMP_LOG"
  printf '{}\n'
  exit 0
elif [ "$*" = '--profile glen-harness models cursor --json' ]; then
  [ -f "$HOME/.omp/natives/prewarmed" ] || exit 96
fi
exec "$SMOKE_OMP_BASE" "$@"
EOF
export PATH="$WORK/bin:$PATH"
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
all_scoped() {
  jq -es 'length > 0 and all(.[]; .argv[0:2] == ["--profile","glen-harness"])' "$FAKE_OMP_LOG"
}
tree_contents() {
  (cd "$1"; find . -print | LC_ALL=C sort; find . -type f -exec shasum -a 256 {} + | LC_ALL=C sort)
}

# Stubs prevent builds and glen's doctor/install from ever reaching real omp.
cat > "$WORK/bin/cargo" <<'EOF'
#!/bin/bash
set -eu
printf 'cargo %s\n' "$*" >> "$FAKE_TOOL_CALLS"
[ "$1" = build ]
EOF
cat > "$WORK/bin/glen" <<'EOF'
#!/bin/bash
set -eu
printf 'glen %s\n' "$*" >> "$FAKE_TOOL_CALLS"
case "$*" in
  --version) printf 'glen/fixture\n' ;;
  '--profile glen-harness install-extension --json') printf '{}\n' ;;
  '--profile glen-harness doctor --json')
    jq -cn --argjson current "${FAKE_GLEN_CURRENT:-true}" '{data:{extension:{state:"fixture",current:$current}}}'
    ;;
  *) exit 99 ;;
esac
EOF
chmod +x "$WORK/bin/omp" "$WORK/bin/cargo" "$WORK/bin/glen"

# Run the actual launcher, replacing only the OS sandbox program in this offline
# routing suite. Local config invocations must not pass through either stub.
cat > "$WORK/bin/sandbox-exec" <<'EOF'
#!/bin/bash
set -eu
[ "$1" = -f ] && [ -f "$2" ] || exit 99
shift 2
export FAKE_OMP_SANDBOX_ACTIVE=1
jq -cn --arg agent "$OMP_PROFILE_AGENT_DIR" --arg root "$OMP_SANDBOX_RUN_ROOT" \
  --arg cwd "$(pwd -P)" --args \
  '{agent:$agent,root:$root,cwd:$cwd,argv:$ARGS.positional}' -- "${@:2}" >> "$FAKE_OMP_LAUNCH_LOG"
exec "$@"
EOF
cat > "$WORK/bin/bwrap" <<'EOF'
#!/bin/bash
set -eu
while [ "$1" != -- ]; do shift; done
shift
export FAKE_OMP_SANDBOX_ACTIVE=1
jq -cn --arg agent "$OMP_PROFILE_AGENT_DIR" --arg root "$OMP_SANDBOX_RUN_ROOT" \
  --arg cwd "$(pwd -P)" --args \
  '{agent:$agent,root:$root,cwd:$cwd,argv:$ARGS.positional}' -- "${@:2}" >> "$FAKE_OMP_LAUNCH_LOG"
exec "$@"
EOF
chmod +x "$WORK/bin/sandbox-exec" "$WORK/bin/bwrap" "$WORK/bin/omp-base"

new_fixture() {
  CASE_DIR="$(mktemp -d "$WORK/case.XXXXXX")"
  TEST_HOME="$CASE_DIR/home" TEST_CONFIG_DIR="${1:-.omp}"
  TEST_ROOT="$TEST_HOME/${TEST_CONFIG_DIR#/}"
  TEST_AGENT="$TEST_ROOT/profiles/glen-harness/agent"
  mkdir -p "$TEST_HOME" "$CASE_DIR/tmp" "$CASE_DIR/state" "$TEST_ROOT/agent/extensions" \
    "$TEST_ROOT/agent/sessions" "$TEST_ROOT/natives"
  printf 'owner config\n' > "$TEST_ROOT/agent/config.yml"
  printf 'owner mcp\n' > "$TEST_ROOT/agent/mcp.json"
  printf 'owner extension\n' > "$TEST_ROOT/agent/extensions/existing"
  printf 'owner session\n' > "$TEST_ROOT/agent/sessions/existing"
  printf 'owner native\n' > "$TEST_ROOT/natives/existing"
  export FAKE_OMP_TEST_HOME="$TEST_HOME" FAKE_OMP_AGENT_DIR="$TEST_AGENT"
  export FAKE_OMP_STATE="$CASE_DIR/state" FAKE_OMP_LOG="$CASE_DIR/omp.jsonl"
  export FAKE_OMP_WRITES="$CASE_DIR/writes" FAKE_TOOL_CALLS="$CASE_DIR/tool-calls"
  export FAKE_OMP_LAUNCH_LOG="$CASE_DIR/launcher-calls.jsonl"
  unset FAKE_OMP_VERSION FAKE_OMP_REPORTED_DIR FAKE_OMP_REJECT_KEY FAKE_OMP_ERROR_KEY FAKE_OMP_MUTATE FAKE_GLEN_CURRENT
  unset FAKE_OMP_DB_FILE
  unset FAKE_OMP_PREWARM_FAIL FAKE_OMP_SANDBOX_ACTIVE
  : > "$FAKE_OMP_LOG"
  : > "$FAKE_OMP_LAUNCH_LOG"
  : > "$FAKE_OMP_WRITES"
  : > "$FAKE_TOOL_CALLS"
  # Canned config responses keyed by the exact registered setting ID.
  cat > "$FAKE_OMP_STATE/overlay.json" <<'EOF'
{"modelRoles":{"default":"cursor/gemini-4-argon","task":"cursor/task-model","custom":"cursor/custom-model"},"tools.approvalMode":"always-ask","tools.approval":{"write":"deny","glen_reply":"allow"},"tools.xdev":false,"tools.restrictedFolders":["~"],"fetch.enabled":false,"web_search.enabled":false,"bash.enabled":false,"eval.py":false,"eval.js":false,"browser.enabled":false,"lsp.enabled":false,"mcp.enableProjectConfig":false,"mcp.startupTimeoutMs":15000}
EOF
  cp "$FAKE_OMP_STATE/overlay.json" "$FAKE_OMP_STATE/settings.json"
  printf '%s\n' '{"models":[{"selector":"cursor/gemini-4-argon"}]}' > "$FAKE_OMP_STATE/models.json"
}
ready_assets() {
  mkdir -p "$TEST_AGENT/skills/mnemosyne-glen"
  cp "$REPO_ROOT/.claude/omp/mcp.json" "$TEST_AGENT/mcp.json"
  cp "$REPO_ROOT/.claude/omp/skills/mnemosyne-glen/SKILL.md" "$TEST_AGENT/skills/mnemosyne-glen/SKILL.md"
}
setup_run() {
  # Every caller creates a new fixture. Isolate XDG and native overrides too.
  run env HOME="$TEST_HOME" TMPDIR="$CASE_DIR/tmp" PI_CONFIG_DIR="$TEST_CONFIG_DIR" \
    XDG_DATA_HOME="$CASE_DIR/xdg-data" XDG_STATE_HOME="$CASE_DIR/xdg-state" XDG_CACHE_HOME="$CASE_DIR/xdg-cache" \
    PI_NATIVES_DIR='' GLEN_BIN="$WORK/bin/glen" /bin/bash "$@"
}

new_fixture
setup_run "$WORK/bin/omp" --profile glen-harness --version
assert_exit 'native prewarm control version command succeeds' 0
assert 'version control leaves native cache cold' test ! -e "$TEST_HOME/.omp/natives/prewarmed"

new_fixture
# Reverse the live list's order and exercise both selector and provider/id forms:
# preference order, rather than API order, must choose Argon when both exist.
printf '%s\n' '{"models":[{"selector":"cursor/gemini-3.8-flash"},{"selector":"cursor/other"},{"provider":"cursor","id":"gemini-4-argon"}]}' > "$FAKE_OMP_STATE/models.json"
jq '.modelRoles.default = "cursor/old" | .["tools.xdev"] = true' "$FAKE_OMP_STATE/settings.json" > "$CASE_DIR/drift.json"
cp "$CASE_DIR/drift.json" "$FAKE_OMP_STATE/settings.json"
setup_run "$SETUP"
assert_exit 'full setup succeeds with record-only modelRoles CLI' 0
assert 'full setup preserves other model roles' jq -e '.modelRoles == {default:"cursor/gemini-4-argon",task:"cursor/task-model",custom:"cursor/custom-model"}' "$FAKE_OMP_STATE/settings.json"
assert 'full setup applies other overlay settings' jq -e '.["tools.xdev"] == false' "$FAKE_OMP_STATE/settings.json"
assert 'full setup queries bare modelRoles key' jq -es 'any(.[]; .argv == ["--profile","glen-harness","config","get","modelRoles","--json"])' "$FAKE_OMP_LOG"
assert 'full setup sets bare modelRoles key with whole record' jq -es 'any(.[]; .argv[2:5] == ["config","set","modelRoles"] and (.argv[5] | fromjson | .task == "cursor/task-model"))' "$FAKE_OMP_LOG"
assert 'full setup makes only harness-scoped omp calls' all_scoped
assert 'full setup installs MCP asset' cmp -s "$REPO_ROOT/.claude/omp/mcp.json" "$TEST_AGENT/mcp.json"
assert 'full setup installs skill asset' cmp -s "$REPO_ROOT/.claude/omp/skills/mnemosyne-glen/SKILL.md" "$TEST_AGENT/skills/mnemosyne-glen/SKILL.md"
assert_contains 'full setup reaches build stub' 'cargo build --release --locked' "$FAKE_TOOL_CALLS"
assert_contains 'both preferred models present chooses Argon visibly' 'chose cursor/gemini-4-argon from preference list' "$STDOUT"
assert_contains 'Argon selection reports its medium thinking level' 'chose cursor/gemini-4-argon from preference list, thinking medium' "$STDOUT"
# shellcheck disable=SC2016 # jq binds its own $calls variable.
assert 'full setup fetches models before setting config' jq -es '
  map(.argv[2:4] | join(" ")) as $calls |
  ($calls | index("models cursor")) < ($calls | index("config set"))' "$FAKE_OMP_LOG"
# shellcheck disable=SC2016 # jq receives $agent through --arg.
assert 'setup launches only the model query with explicit profile and cwd' jq -es --arg agent "$TEST_AGENT" \
  'length==1 and all(.[]; .argv==["--profile","glen-harness","models","cursor","--json"] and .agent==$agent and .root==.cwd)' "$FAKE_OMP_LAUNCH_LOG"
assert 'setup removes model query scratch root after the call' test ! -e "$(jq -r .root "$FAKE_OMP_LAUNCH_LOG")"
assert 'setup prewarms native cache before sandboxed models' test -f "$TEST_HOME/.omp/natives/prewarmed"
# shellcheck disable=SC2016 # jq binds its own $calls variable.
assert 'setup native prewarm precedes model catalog query' jq -es '
  map(.argv[2]) as $calls | ($calls | index("toks")) < ($calls | index("models"))' "$FAKE_OMP_LOG"
# A modified worktree launcher cannot replace the captured main version.
if [ "$(uname -s)" = Darwin ]; then launcher_name=omp-sandboxed.sh; else launcher_name=omp-sandboxed-linux.sh; fi
printf '#!/bin/bash\nexit 99\n' > "$ENGINE_FIXTURE/.claude/omp/$launcher_name"
setup_run "$SETUP" --check
assert_exit 'setup model fetch prefers main over modified worktree launcher' 0
assert 'setup using main launcher has no fallback warning' test ! -s "$STDERR"

new_fixture
export FAKE_OMP_PREWARM_FAIL=1
setup_run "$SETUP" --dry-run
assert_exit 'setup stops if native cache prewarm fails' 97
assert 'failed prewarm never queries models or sets config' jq -es \
  'all(.[]; .argv[2]!="models" and .argv[2:4]!=["config","set"])' "$FAKE_OMP_LOG"
assert 'failed prewarm never invokes sandboxed payload' test ! -s "$FAKE_OMP_LAUNCH_LOG"

new_fixture
printf '%s\n' '{"models":[{"provider":"cursor","id":"gemini-3.8-flash"},{"selector":"cursor/other"}]}' > "$FAKE_OMP_STATE/models.json"
setup_run "$SETUP"
assert_exit 'missing Argon falls back to Flash' 0
assert 'fallback sets Flash and preserves other model roles' jq -e \
  '.modelRoles == {default:"cursor/gemini-3.8-flash",task:"cursor/task-model",custom:"cursor/custom-model"}' "$FAKE_OMP_STATE/settings.json"
assert_contains 'fallback model decision is visible' 'chose cursor/gemini-3.8-flash from preference list' "$STDOUT"
assert_contains 'Flash fallback also reports medium thinking' 'chose cursor/gemini-3.8-flash from preference list, thinking medium' "$STDOUT"
assert 'fallback really writes chosen model' jq -es 'any(.[];
  .argv[2:5] == ["config","set","modelRoles"] and
  (.argv[5] | fromjson | .default == "cursor/gemini-3.8-flash"))' "$FAKE_OMP_LOG"
# The subsequent check must display the same resolved model without any drift.
setup_run "$SETUP" --check
assert_exit 'fallback model check succeeds' 0
assert 'fallback check report shows resolved model' grep -Eq '^model[[:space:]]+cursor/gemini-3.8-flash$' "$STDOUT"

new_fixture
printf '%s\n' '{"models":[{"selector":"cursor/grok-4.7"},{"selector":"cursor/other"}]}' > "$FAKE_OMP_STATE/models.json"
setup_run "$SETUP"
assert_exit 'Argon and Flash both missing falls back to Grok at high' 0
assert 'Grok fallback sets Grok and preserves other model roles' jq -e \
  '.modelRoles == {default:"cursor/grok-4.7",task:"cursor/task-model",custom:"cursor/custom-model"}' "$FAKE_OMP_STATE/settings.json"
assert_contains 'Grok fallback decision is visible' 'chose cursor/grok-4.7 from preference list, thinking high' "$STDOUT"
assert 'Grok fallback really writes chosen model' jq -es 'any(.[];
  .argv[2:5] == ["config","set","modelRoles"] and
  (.argv[5] | fromjson | .default == "cursor/grok-4.7"))' "$FAKE_OMP_LOG"

new_fixture
printf '%s\n' '{"models":[{"selector":"cursor/other"}]}' > "$FAKE_OMP_STATE/models.json"
setup_run "$SETUP"
assert_exit 'none of the preferred models available fails' 1
assert_contains 'missing preferences reason explains none found' 'none of the preferred models' "$STDERR"
assert_contains 'missing preferences reason names Argon' 'cursor/gemini-4-argon' "$STDERR"
assert_contains 'missing preferences reason names Flash' 'cursor/gemini-3.8-flash' "$STDERR"
assert_contains 'missing preferences reason names Grok' 'cursor/grok-4.7' "$STDERR"
assert 'missing preferences never set config' jq -es \
  'all(.[]; .argv[2:4] != ["config","set"])' "$FAKE_OMP_LOG"
assert 'missing preferences never install MCP' test ! -e "$TEST_AGENT/mcp.json"
assert 'missing preferences never install skill' test ! -e "$TEST_AGENT/skills/mnemosyne-glen/SKILL.md"
assert 'missing preferences never build or install extension' test ! -s "$FAKE_TOOL_CALLS"
assert 'missing preferences still use only harness-scoped calls' all_scoped

new_fixture
printf '%s\n' '{"models":[{"selector":"cursor/gemini-4-argon"},{"selector":"cursor/gemini-3.8-flash"},{"selector":"cursor/explicit"}]}' > "$FAKE_OMP_STATE/models.json"
setup_run "$SETUP" --model cursor/explicit
assert_exit 'explicit model bypasses preference order' 0
assert 'explicit model is the configured default' jq -e '.modelRoles.default == "cursor/explicit"' "$FAKE_OMP_STATE/settings.json"
assert_contains 'explicit model decision is visible' 'model: cursor/explicit (explicit --model)' "$STDOUT"

new_fixture
printf '%s\n' '{"models":[{"selector":"cursor/gemini-4-argon"},{"selector":"cursor/gemini-3.8-flash"}]}' > "$FAKE_OMP_STATE/models.json"
setup_run "$SETUP" --model cursor/missing
assert_exit 'missing explicit model does not fall back' 1
assert_contains 'missing explicit model keeps exact-id diagnostic' 'model cursor/missing is not listed' "$STDERR"

new_fixture
printf '%s\n' '{"models":[]}' > "$FAKE_OMP_STATE/models.json"
setup_run "$SETUP" --dry-run
assert_exit 'empty model list preserves missing-login exit' 2
assert_contains 'empty model list leaves preference list unvalidated' 'no live Cursor login yet; preference list unvalidated' "$STDOUT"
assert_contains 'empty model list retains owner login instruction' "Owner action: run 'omp --profile glen-harness login cursor'" "$STDOUT"
assert 'empty model list produces no unavailable-model error' test ! -s "$STDERR"

# Negative control: execute a copy in memory with only the queried key reverted.
# $0 still identifies the real script, so repository resolution is unchanged.
new_fixture
reverted="$(sed 's/^KEYS=(modelRoles /KEYS=(modelRoles.default /' "$SETUP")"
setup_run -c "$reverted" "$SETUP" --dry-run
assert_exit 'reverted modelRoles.default query fails' 43
assert_contains 'reverted key rejected by fake omp' 'Unknown setting: modelRoles.default' "$STDERR"
assert 'reverted key actually reached fake omp' jq -es 'any(.[]; .argv[4] == "modelRoles.default")' "$FAKE_OMP_LOG"
assert 'reverted key failure performs no config sets' test "$(grep -c '^config set ' "$FAKE_OMP_WRITES" || true)" -eq 0

new_fixture
export FAKE_OMP_ERROR_KEY=modelRoles
setup_run "$SETUP" --dry-run
assert 'modelRoles error response fails closed' test "$CODE" -ne 0
assert_contains 'modelRoles error response names record problem' 'modelRoles must be a record' "$STDERR"

# Restoring the old safety query must fail against the same scoped-only fake.
new_fixture
# shellcheck disable=SC2016 # Keep command substitution literal in the reverted script.
reverted="$(sed '/^AGENT_DIR=.*profile_omp.*config path/a\
legacy_default_agent_dir="$(omp_run profile config path)"
' "$SETUP")"
setup_run -c "$reverted" "$SETUP" --dry-run
assert_exit 'reverted default-profile query fails' 42
assert_contains 'fake rejects default-profile safety query' 'unscoped omp invocation refused' "$STDERR"
assert 'unscoped invocation detector catches reverted query' jq -es 'any(.[]; .argv[0:2] != ["--profile","glen-harness"])' "$FAKE_OMP_LOG"

new_fixture /custom-omp
setup_run "$SETUP" --dry-run
assert_exit 'leading-slash PI_CONFIG_DIR stays under scratch HOME' 0
assert 'custom config directory still scopes every call' all_scoped
# shellcheck disable=SC2016 # jq receives $agent through --arg.
assert 'setup passes resolved custom profile directory into launcher' jq -es --arg agent "$TEST_AGENT" \
  'length==1 and all(.[]; .agent==$agent)' "$FAKE_OMP_LAUNCH_LOG"

# Setup may configure local files directly, but it cannot query models without
# an OS sandbox. The allowlisted PATH contains no sandbox-exec or bwrap.
mkdir "$WORK/no-sandbox-bin"
for tool in date dirname git jq shasum uname mktemp rm find stat awk sort env omp mkdir cmp grep basename; do
  ln -s "$(command -v "$tool")" "$WORK/no-sandbox-bin/$tool"
done
new_fixture
PATH="$WORK/no-sandbox-bin" setup_run "$SETUP" --dry-run
assert_exit 'setup model fetch without sandbox fails closed' 1
if [ "$(uname -s)" = Darwin ]; then missing_tool=sandbox-exec; else missing_tool=bwrap; fi
assert_contains 'setup missing sandbox reason names tool' "$missing_tool not on PATH" "$STDERR"
assert 'setup without sandbox never queries models or sets config' jq -es \
  'all(.[]; .argv[2]!="models" and .argv[2:4]!=["config","set"])' "$FAKE_OMP_LOG"
assert 'setup without sandbox never executes launcher payload' test ! -s "$FAKE_OMP_LAUNCH_LOG"

# HOME aliases are allowed; only symlinks below its physical location are unsafe.
# Cover both the nearest-existing-ancestor fallback and an existing agent dir.
for existing in no yes; do
  new_fixture
  [ "$existing" != yes ] || ready_assets
  physical_home="$TEST_HOME"
  ln -s "$physical_home" "$CASE_DIR/home-alias"
  TEST_HOME="$CASE_DIR/home-alias"
  setup_run "$SETUP" --dry-run
  assert_exit "symlinked HOME existing=$existing dry-run succeeds" 0
  assert "symlinked HOME existing=$existing keeps harness scope" all_scoped
  # shellcheck disable=SC2016 # jq expands its own --arg variable.
  assert "symlinked HOME existing=$existing uses physical HOME for omp" jq -es \
    --arg home "$physical_home" 'length > 0 and all(.[]; .home==$home)' "$FAKE_OMP_LOG"
done

new_fixture
ready_assets
physical_home="$TEST_HOME"
cp -R "$TEST_AGENT" "$CASE_DIR/outside-agent"
rm -r "$TEST_AGENT"
ln -s "$CASE_DIR/outside-agent" "$TEST_AGENT"
ln -s "$physical_home" "$CASE_DIR/home-alias"
TEST_HOME="$CASE_DIR/home-alias"
before="$(tree_contents "$CASE_DIR")"
setup_run "$SETUP" --dry-run
assert_exit 'symlinked HOME still refuses symlink at agent component' 1
assert_contains 'symlinked HOME component refusal names physical agent path' "$TEST_AGENT" "$STDERR"
assert 'symlinked HOME component refusal precedes omp initialization' test ! -s "$FAKE_OMP_LOG"
assert_eq 'symlinked HOME component refusal leaves fixture unchanged' "$(tree_contents "$CASE_DIR")" "$before"

for version in omp/18.14.0 18.14.0 omp/18.15.0; do
  new_fixture
  ready_assets
  export FAKE_OMP_VERSION="$version"
  setup_run "$SETUP" --check
  assert_exit "check version $version is informational" 0
  assert "check version $version scopes every call including version" all_scoped
  assert_contains "check version $version fingerprint stable" 'fingerprint-unchanged' "$STDOUT"
  if [ "$version" = omp/18.15.0 ]; then
    assert_contains 'version mismatch still warns' 'WARN: expected omp 18.14.0' "$STDERR"
  else
    assert "matching version $version has no warning" test ! -s "$STDERR"
  fi
done
new_fixture
ready_assets
export FAKE_OMP_VERSION=omp/18.15.0 FAKE_GLEN_CURRENT=false
setup_run "$SETUP" --check
assert_exit 'version warning does not hide another check failure' 1
assert_contains 'failed check retains version warning' 'WARN: expected omp 18.14.0' "$STDERR"

for component in .omp .omp/profiles .omp/profiles/glen-harness .omp/profiles/glen-harness/agent \
  .omp/profiles/glen-harness/agent/config.yml .omp/profiles/glen-harness/agent/mcp.json \
  .omp/profiles/glen-harness/agent/skills .omp/profiles/glen-harness/agent/skills/mnemosyne-glen \
  .omp/profiles/glen-harness/agent/skills/mnemosyne-glen/SKILL.md; do
  new_fixture
  ready_assets
  printf 'fixture config\n' > "$TEST_AGENT/config.yml"
  target="$TEST_HOME/$component"
  cp -R "$target" "$CASE_DIR/outside"
  rm -rf "$target"
  ln -s "$CASE_DIR/outside" "$target"
  before="$(tree_contents "$CASE_DIR")"
  setup_run "$SETUP"
  assert_exit "setup refuses symlink at $component" 1
  assert_contains "symlink diagnostic names $component" "$target" "$STDERR"
  assert_eq "symlink $component refuses before even Settings.init" "$(cat "$FAKE_OMP_LOG")" ''
  after="$(tree_contents "$CASE_DIR")"
  assert_eq "symlink $component leaves fixture contents unchanged" "$after" "$before"
  if [ "$after" != "$before" ]; then
    diff -u <(printf '%s\n' "$before") <(printf '%s\n' "$after") >&2 || true
  fi
done

new_fixture
mkdir -p "$CASE_DIR/physical-parent"
ln -s "$CASE_DIR/physical-parent" "$TEST_HOME/alias"
TEST_CONFIG_DIR=alias/.omp
setup_run "$SETUP"
assert_exit 'setup refuses differing physical profile path' 1
assert_contains 'physical path mismatch diagnostic' 'real path differs' "$STDERR"
assert 'physical path mismatch precedes omp initialization' test ! -s "$FAKE_OMP_LOG"
assert_eq 'physical path mismatch leaves symlink target empty' "$(find "$CASE_DIR/physical-parent" -mindepth 1 -print)" ''

new_fixture
export FAKE_OMP_REPORTED_DIR="$TEST_ROOT/agent"
setup_run "$SETUP"
assert_exit 'setup refuses reported default agent directory' 1
assert_contains 'default directory collision diagnostic' 'resolves to the default profile' "$STDERR"
assert 'directory collision never queries default profile' all_scoped
assert 'directory collision performs no config sets' test "$(grep -c '^config set ' "$FAKE_OMP_WRITES" || true)" -eq 0

for component in config.yml mcp.json extensions sessions; do
  new_fixture
  ready_assets
  export FAKE_OMP_MUTATE="$component"
  setup_run "$SETUP" --check
  assert_exit "default fingerprint $component mutation fails" 1
  assert_contains "default fingerprint names changed $component" "default-profile fingerprint changed: $component" "$STDERR"
  assert 'fingerprint mismatch reports no unchanged claim' test "$(grep -c 'fingerprint-unchanged.*yes' "$STDOUT" || true)" -eq 0
done
for component in run natives; do
  new_fixture
  ready_assets
  export FAKE_OMP_MUTATE="$component"
  setup_run "$SETUP" --check
  assert_exit "documented shared $component directory remains outside fingerprint" 0
  assert_contains "shared $component mutation leaves default fingerprint unchanged" 'fingerprint-unchanged' "$STDOUT"
  assert "shared $component mutation reports no fingerprint error" test ! -s "$STDERR"
  if [ "$component" = run ]; then path="$TEST_ROOT/run/daemons/global/added"; else path="$TEST_ROOT/natives/added"; fi
  assert "shared $component fixture really changed" test -f "$path"
done

# Wrap the existing scoped fake only for database mutations. No SQLite content
# or timestamp belongs in this fingerprint: existence plus byte size is enough.
mkdir "$WORK/db-bin"
export SMOKE_DB_OMP="$WORK/bin/omp"
cat > "$WORK/db-bin/omp" <<'EOF'
#!/bin/bash
set -euo pipefail
if [ "$*" = '--profile glen-harness models cursor --json' ]; then
  database="$FAKE_OMP_TEST_HOME/${PI_CONFIG_DIR:-.omp}/agent/${FAKE_OMP_DB_FILE:-agent.db}"
  case "$FAKE_OMP_DB_CHANGE" in
    size | appear) printf 'database grew\n' >> "$database" ;;
    disappear) rm "$database" ;;
    mtime) touch -t 200101010000 "$database" ;;
    content) printf 'DATABASE fixture\n' > "$database" ;;
    *) exit 99 ;;
  esac
fi
exec "$SMOKE_DB_OMP" "$@"
EOF
chmod +x "$WORK/db-bin/omp"
for change in size appear disappear mtime content; do
  new_fixture
  ready_assets
  database="$TEST_ROOT/agent/agent.db"
  if [ "$change" != appear ]; then
    printf 'database fixture\n' > "$database"
    touch -t 200001010000 "$database"
  fi
  export FAKE_OMP_DB_CHANGE="$change"
  PATH="$WORK/db-bin:$PATH" setup_run "$SETUP" --check
  case "$change" in
    size | appear | disappear)
      assert_exit "default fingerprint agent.db $change change fails" 1
      assert_contains "default fingerprint agent.db detects $change" 'default-profile fingerprint changed: agent.db' "$STDERR"
      assert "database $change mismatch reports no unchanged claim" test "$(grep -c 'fingerprint-unchanged.*yes' "$STDOUT" || true)" -eq 0
      ;;
    mtime | content)
      assert_exit "default fingerprint agent.db ignores $change-only change" 0
      assert_contains "database $change-only change leaves fingerprint unchanged" 'fingerprint-unchanged' "$STDOUT"
      assert "database $change-only change reports no fingerprint error" test ! -s "$STDERR"
      ;;
  esac
  case "$change" in
    size) assert_eq 'database size fixture really grew' "$(wc -c < "$database" | tr -d ' ')" 31 ;;
    appear) assert_contains 'database appearance fixture really appeared' 'database grew' "$database" ;;
    disappear) assert 'database disappearance fixture really disappeared' test ! -e "$database" ;;
    mtime)
      assert 'database mtime fixture really changed' test "$(perl -e 'print((stat shift)[9])' "$database")" -gt 970000000
      assert_eq 'database mtime change preserves size' "$(wc -c < "$database" | tr -d ' ')" 17
      ;;
    content)
      assert_contains 'database content fixture really changed' 'DATABASE fixture' "$database"
      assert_eq 'database content change preserves size' "$(wc -c < "$database" | tr -d ' ')" 17
      ;;
  esac
  assert 'database fingerprint test keeps every omp call scoped' all_scoped
done

# A sidecar alone can hold new SQLite state while agent.db stays byte-for-byte
# unchanged. Reuse the same mutation helper and the existing component diagnostic.
for sidecar in agent.db-wal agent.db-shm; do
  for change in size appear disappear; do
    new_fixture
    ready_assets
    database="$TEST_ROOT/agent/agent.db"
    printf 'unchanged database\n' > "$database"
    cp "$database" "$CASE_DIR/database-before"
    if [ "$change" != appear ]; then
      printf 'database fixture\n' > "$TEST_ROOT/agent/$sidecar"
    fi
    export FAKE_OMP_DB_FILE="$sidecar" FAKE_OMP_DB_CHANGE="$change"
    PATH="$WORK/db-bin:$PATH" setup_run "$SETUP" --check
    assert_exit "default fingerprint $sidecar $change change fails" 1
    assert_contains "default fingerprint detects $sidecar $change" 'default-profile fingerprint changed: agent.db' "$STDERR"
    assert "$sidecar $change leaves main database unchanged" cmp -s "$database" "$CASE_DIR/database-before"
    assert "$sidecar $change mismatch reports no unchanged claim" test "$(grep -c 'fingerprint-unchanged.*yes' "$STDOUT" || true)" -eq 0
    case "$change" in
      size) assert_eq "$sidecar really grew" "$(wc -c < "$TEST_ROOT/agent/$sidecar" | tr -d ' ')" 31 ;;
      appear) assert_contains "$sidecar really appeared" 'database grew' "$TEST_ROOT/agent/$sidecar" ;;
      disappear) assert "$sidecar really disappeared" test ! -e "$TEST_ROOT/agent/$sidecar" ;;
    esac
    assert "$sidecar $change test keeps every omp call scoped" all_scoped
  done
done

printf '%s assertions, %s failures\n' "$CASES" "$FAILURES"
[ "$FAILURES" -eq 0 ]
