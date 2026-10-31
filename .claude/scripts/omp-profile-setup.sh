#!/usr/bin/env bash
# Set up the isolated omp reviewer/debugger profile; reruns sync only differences.
# Usage: omp-profile-setup.sh [--model cursor/<id>] [--check] [--dry-run]
set -euo pipefail

PROFILE=glen-harness
# $0, not BASH_SOURCE[0]: a test negative control runs this script's text
# through `bash -c "$text" "$0_override" ...`, where BASH_SOURCE is unset.
# shellcheck source=.claude/omp/model-ladder.sh
. "$(cd "$(dirname "$0")/../omp" && pwd -P)/model-ladder.sh"
DEFAULT_MODEL_PREFERENCE=("${MODEL_LADDER[@]}")
DEFAULT_THINKING_PREFERENCE=("${THINKING_LADDER[@]}")
MODEL="${DEFAULT_MODEL_PREFERENCE[0]}"
THINKING="${DEFAULT_THINKING_PREFERENCE[0]}"
EXPLICIT_MODEL=0
CHECK=0
DRY_RUN=0
say() { printf '%s\n' "$*"; }
die() { say "ERROR: $*" >&2; exit 1; }

while [ "$#" -gt 0 ]; do
  case "$1" in
    --model)
      [ "$#" -ge 2 ] || die '--model needs cursor/<id>'
      MODEL="$2"
      EXPLICIT_MODEL=1
      shift 2
      ;;
    --check) CHECK=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --help | -h) sed -n '2,3s/^# //p' "$0"; exit 0 ;;
    *) die "Unknown arg: $1 (use --help)" ;;
  esac
done
case "$MODEL" in cursor/?*) ;; *) die '--model must be cursor/<id>' ;; esac

# HOME's ancestry may contain system aliases such as macOS /var -> /private/var.
# Anchor both our baseline and child directory resolution to the same physical HOME;
# symlinks below it still go through profile_path_guard before any omp invocation.
PROFILE_HOME="$(cd "$HOME" && pwd -P)" || die 'Could not resolve HOME directory'
# PI_CONFIG_DIR is a directory name under HOME, including when it starts with /.
OMP_ROOT="$PROFILE_HOME/${PI_CONFIG_DIR:-.omp}"
for tool in omp jq shasum; do
  command -v "$tool" >/dev/null 2>&1 || die "Required tool missing: $tool"
done
# Match node:path.join's lexical normalization, including a leading slash in
# PI_CONFIG_DIR (joined under HOME, never an absolute override).
OMP_ROOT="$(jq -nr --arg path "$OMP_ROOT" '
  $path | split("/") | reduce .[] as $part ([];
    if $part == "" or $part == "." then .
    elif $part == ".." then .[:-1] else . + [$part] end) | "/" + join("/")')"
EXPECTED_AGENT_DIR="$OMP_ROOT/profiles/$PROFILE/agent"
DEFAULT_AGENT_DIR="$OMP_ROOT/agent"
MCP_TARGET="$EXPECTED_AGENT_DIR/mcp.json"
SKILL_TARGET="$EXPECTED_AGENT_DIR/skills/mnemosyne-glen/SKILL.md"

profile_path_guard() {
  local target path ancestor suffix real
  # config path initializes settings too. Check the expected chain BEFORE the
  # first omp invocation, including config.yml and every asset parent directory.
  for target in "$EXPECTED_AGENT_DIR" "$EXPECTED_AGENT_DIR/config.yml" "$MCP_TARGET" "$SKILL_TARGET"; do
    path="$target"
    while :; do
      [ ! -L "$path" ] || die "Refusing symlink in harness profile path: $path"
      [ "$path" != "$OMP_ROOT" ] || break
      path="$(dirname "$path")"
    done
  done
  # First-time setup need not have an agent directory yet. Resolve its nearest
  # existing parent without creating anything and append the missing suffix.
  ancestor="$EXPECTED_AGENT_DIR" suffix=''
  while [ ! -d "$ancestor" ]; do
    [ ! -e "$ancestor" ] || die "Harness profile parent is not a directory: $ancestor"
    suffix="/$(basename "$ancestor")$suffix"
    ancestor="$(dirname "$ancestor")"
  done
  real="$(cd "$ancestor" && pwd -P)$suffix"
  [ "$real" = "$EXPECTED_AGENT_DIR" ] || die "Harness profile real path differs from expected path: $EXPECTED_AGENT_DIR"
}
profile_path_guard

ENGINE_ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
MAIN="$(dirname "$(git -C "$ENGINE_ROOT" rev-parse --path-format=absolute --git-common-dir)")"
OVERLAY="$ENGINE_ROOT/.claude/omp/glen-harness.yml"
MCP_SOURCE="$ENGINE_ROOT/.claude/omp/mcp.json"
SKILL_SOURCE="$ENGINE_ROOT/.claude/omp/skills/mnemosyne-glen/SKILL.md"
SKILL_MARKER='<!-- Managed by omp-profile-setup.sh: mnemosyne-glen -->'

# Root natives/ is a shared, cross-profile native-binary cache populated by omp
# invocations under any profile, intentionally outside the default-profile fingerprint.
# Shared run/daemons/global and run/session-owners (possibly under
# XDG_STATE_HOME/omp/run) are runtime state, intentionally not fingerprinted.
COMPONENTS=(config.yml mcp.json agent.db extensions sessions)
fingerprint() {
  local component="$1" path="$DEFAULT_AGENT_DIR/$1" database
  case "$component" in
    config.yml | mcp.json)
      if [ -f "$path" ]; then shasum -a 256 < "$path"; else say absent; fi
      ;;
    agent.db)
      # omp utils/src/dirs.ts:getAgentDbPath puts settings/auth SQLite state
      # directly under agent/. Include SQLite WAL/SHM sidecars, since pending
      # writes need not change the main DB. Compare existence/bytes, not content/mtime.
      for database in "$path" "$path-wal" "$path-shm"; do
        printf '%s ' "${database##*/}"
        if [ ! -f "$database" ]; then say absent
        elif [ "$(uname -s)" = Darwin ]; then stat -f '%z' "$database" || return 1
        else stat -c '%s' "$database" || return 1
        fi
      done
      ;;
    extensions)
      if [ -d "$path" ]; then
        find "$path" -mindepth 1 -maxdepth 1 -print | LC_ALL=C sort
      else
        say absent
      fi
      ;;
    sessions)
      if [ ! -d "$path" ]; then say '0 0'; return; fi
      if [ "$(uname -s)" = Darwin ]; then
        find "$path" -type f -exec stat -f '%m' {} + || return 1
      else
        find "$path" -type f -exec stat -c '%Y' {} + || return 1
      fi | awk 'BEGIN { count=0; newest=0 } { count++; if ($1>newest) newest=$1 } END { print count, newest }'
      ;;
  esac
}

BEFORE=()
for component in "${COMPONENTS[@]}"; do
  before="$(fingerprint "$component")" || die "Default-profile fingerprint could not be read: $component"
  BEFORE+=("$before")
done
TMPROOT="$(mktemp -d)"
finish() {
  local status="$?" after component index=0 unchanged=yes
  trap - EXIT
  for component in "${COMPONENTS[@]}"; do
    if ! after="$(fingerprint "$component")"; then
      say "ERROR: default-profile fingerprint could not be read: $component" >&2
      status=1 unchanged=no
    elif [ "${BEFORE[$index]}" != "$after" ]; then
      say "ERROR: default-profile fingerprint changed: $component; inspect it manually." >&2
      status=1 unchanged=no
    fi
    index=$((index + 1))
  done
  if [ "$unchanged" = no ]; then
    printf '%-30s %s\n' fingerprint-unchanged no >&2
  elif [ "$CHECK" -eq 1 ]; then
    printf '%-30s %s\n' fingerprint-unchanged yes
  fi
  rm -rf "$TMPROOT"
  exit "$status"
}
trap finish EXIT

# Each call gets a new project-free cwd. Never inherit the caller's overlay or
# active profile; every omp invocation must explicitly select glen-harness.
omp_run() {
  local layer="$1" cwd launcher real_omp
  shift
  (
    cwd="$(mktemp -d "$TMPROOT/omp.XXXXXX")" || exit 1
    cd "$cwd" || exit 1
    cwd="$(pwd -P)"
    if [ "$layer" = sandbox ]; then
      # Fix cycle 5, decision 1/LOW-2: the shared launcher cache, never the
      # per-call sandbox run root ($cwd) the child can write to.
      launcher="$(/bin/bash "$ENGINE_ROOT/.claude/scripts/omp-engine.sh" launcher "$ENGINE_ROOT")" || exit 1
      real_omp="$(command -v omp)" || exit 1
      env -u CLAUDE_CONFIG_DIR -u COPILOT_CUSTOM_INSTRUCTIONS_DIRS -u PI_PY -u PI_JS -u PI_CONFIG_FILES \
        -u PI_CODING_AGENT_DIR -u OMP_CODING_AGENT_DIR -u OMP_PROFILE -u PI_PROFILE \
        HOME="$PROFILE_HOME" OMP_REAL_BIN="$real_omp" OMP_SANDBOX_RUN_ROOT="$cwd" \
        OMP_PROFILE_AGENT_DIR="$AGENT_DIR" "$launcher" "$@" 3>&-
    elif [ "$layer" = overlay ]; then
      env -u CLAUDE_CONFIG_DIR -u PI_PY -u PI_JS -u PI_CONFIG_FILES \
        -u OMP_PROFILE -u PI_PROFILE HOME="$PROFILE_HOME" PI_CONFIG_FILES="$OVERLAY" omp "$@"
    else
      env -u CLAUDE_CONFIG_DIR -u PI_PY -u PI_JS -u PI_CONFIG_FILES \
        -u OMP_PROFILE -u PI_PROFILE HOME="$PROFILE_HOME" omp "$@"
    fi
  )
}
profile_omp() {
  local layer="$1"
  shift
  omp_run "$layer" --profile "$PROFILE" "$@"
}

AGENT_DIR="$(profile_omp profile config path)"
[ -n "$AGENT_DIR" ] || die 'omp returned an empty profile directory'
[ "$AGENT_DIR" != "$DEFAULT_AGENT_DIR" ] || die 'Harness profile resolves to the default profile!'
[ "$AGENT_DIR" = "$EXPECTED_AGENT_DIR" ] || die "Unsafe harness profile directory: $AGENT_DIR"
profile_path_guard

# Refuse owner-authored files before applying any settings, without displaying
# their contents (the MCP file may contain credentials).
asset_guard() {
  local source="$1" target="$2" marker="$3"
  if [ -e "$target" ] && ! cmp -s "$source" "$target" && ! grep -Fq "$marker" "$target"; then
    die "Conflict at $target. Owner action: resolve the owner-authored file manually, then rerun."
  fi
}
asset_guard "$MCP_SOURCE" "$MCP_TARGET" '"mnemosyne"'
asset_guard "$SKILL_SOURCE" "$SKILL_TARGET" "$SKILL_MARKER"

# Resolve the default before syncing settings or installing assets. Primary login
# signal, pending a live owner login (a cached catalog is not a network test).
# models-cli.ts uses online-if-uncached; no usage fallback is actually invoked.
# Native extraction is an authorized, unsandboxed setup operation. Sandboxed omp
# only reads the shared cache; this also applies to --dry-run and --check.
# --version returned without extraction on omp 18.14.0. commands/toks.ts imports
# pi-natives and counts with embedded offline tokenizers; a tiny literal warms it.
profile_omp profile toks glen-native-warmup --json >/dev/null
MODELS="$(profile_omp sandbox models cursor --json)"
MODEL_COUNT="$(printf '%s' "$MODELS" | jq -er '.models | length')"
LOGIN=1
[ "$MODEL_COUNT" -gt 0 ] || LOGIN=0
MODEL_OK=1
model_available() {
  printf '%s' "$MODELS" | jq -e --arg model "$1" \
    'any(.models[]; (.selector // (.provider + "/" + .id)) == $model)' >/dev/null
}
if [ "$LOGIN" -eq 1 ]; then
  if [ "$EXPLICIT_MODEL" -eq 0 ]; then
    MODEL=''
    ladder_index=0
    for candidate in "${DEFAULT_MODEL_PREFERENCE[@]}"; do
      if model_available "$candidate"; then
        MODEL="$candidate"
        THINKING="${DEFAULT_THINKING_PREFERENCE[$ladder_index]}"
        break
      fi
      ladder_index=$((ladder_index + 1))
    done
    if [ -z "$MODEL" ]; then
      say "ERROR: none of the preferred models (${DEFAULT_MODEL_PREFERENCE[*]}) is listed. Available ids:" >&2
      printf '%s' "$MODELS" | jq -r '.models[] | .selector // (.provider + "/" + .id)' >&2
      exit 1
    fi
    [ "$CHECK" -eq 1 ] || say "chose $MODEL from preference list, thinking $THINKING"
  elif ! model_available "$MODEL"; then
    MODEL_OK=0
    say "ERROR: model $MODEL is not listed. Available ids:" >&2
    printf '%s' "$MODELS" | jq -r '.models[] | .selector // (.provider + "/" + .id)' >&2
  else
    [ "$CHECK" -eq 1 ] || say "model: $MODEL (explicit --model)"
  fi
elif [ "$EXPLICIT_MODEL" -eq 0 ] && [ "$CHECK" -eq 0 ]; then
  say "no live Cursor login yet; preference list unvalidated (using $MODEL)"
fi

# Item 10 (fix cycle 4): omp 18.14.0 honours a "<id>:<level>" suffix on a
# model-role value (config/model-resolver.ts splitThinkingSuffix, sdk.ts:2155
# defaultRoleSpec.explicitThinkingLevel) as a persisted default thinking
# level; this is the only profile-level setting, so it is the primary
# mechanism, with the engine's per-dispatch --thinking as belt and braces.
# An explicit --model never gets a ladder level (matches SKILL.md): THINKING
# only carries a real value along the ladder path.
MODEL_ROLE_VALUE="$MODEL"
[ "$EXPLICIT_MODEL" -eq 1 ] || MODEL_ROLE_VALUE="$MODEL:$THINKING"

KEYS=(modelRoles tools.approvalMode tools.approval tools.xdev
  tools.restrictedFolders fetch.enabled web_search.enabled bash.enabled eval.py
  eval.js browser.enabled lsp.enabled mcp.enableProjectConfig mcp.startupTimeoutMs)
CURRENT=() DESIRED=()
DIFFERENCES=0
for key in "${KEYS[@]}"; do
  current="$(profile_omp profile config get "$key" --json | jq -cS .value)"
  if [ "$key" = modelRoles ]; then
    # The CLI registers only the whole record. Preserve all other model roles.
    value_to_set="$(printf '%s' "$current" | jq -ecS --arg model "$MODEL_ROLE_VALUE" '
      if type == "object" then .default = $model else error("modelRoles must be a record") end')"
    current="$(printf '%s' "$current" | jq -cS .default)"
    desired="$(jq -cn --arg model "$MODEL_ROLE_VALUE" '$model')"
  else
    desired="$(profile_omp overlay config get "$key" --json | jq -cS .value)"
    value_to_set="$desired"
  fi
  CURRENT+=("$current")
  DESIRED+=("$desired")
  if [ "$current" = "$desired" ]; then
    say "unchanged $key"
  else
    DIFFERENCES=$((DIFFERENCES + 1))
    if [ "$CHECK" -eq 1 ] || [ "$DRY_RUN" -eq 1 ]; then
      say "would change $key"
    else
      profile_omp profile config set "$key" "$value_to_set" >/dev/null
      say "changed $key"
    fi
  fi
done

sync_asset() {
  local source="$1" target="$2" label="$3"
  if cmp -s "$source" "$target"; then
    say "unchanged $label"
  elif [ "$CHECK" -eq 1 ] || [ "$DRY_RUN" -eq 1 ]; then
    say "would install $label"
  else
    mkdir -p "$(dirname "$target")"
    cp "$source" "$target"
    say "changed $label"
  fi
}
sync_asset "$MCP_SOURCE" "$MCP_TARGET" MCP
sync_asset "$SKILL_SOURCE" "$SKILL_TARGET" skill

if [ "$CHECK" -eq 0 ] && [ "$DRY_RUN" -eq 0 ]; then
  cargo build --release --locked --manifest-path "$MAIN/Cargo.toml"
elif [ "$CHECK" -eq 0 ]; then
  say "would run cargo build --release --locked --manifest-path $MAIN/Cargo.toml"
fi
GLEN_BIN="${GLEN_BIN:-$(command -v glen || printf '%s' "$MAIN/target/release/glen")}"
# doctor invokes omp too, so give glen the same clean environment and fresh cwd.
glen_run() {
  (
    cd "$(mktemp -d "$TMPROOT/glen.XXXXXX")" || exit 1
    env -u CLAUDE_CONFIG_DIR -u PI_PY -u PI_JS -u PI_CONFIG_FILES \
      -u OMP_PROFILE -u PI_PROFILE HOME="$PROFILE_HOME" "$GLEN_BIN" "$@"
  )
}
if [ "$CHECK" -eq 0 ]; then
  if [ "$DRY_RUN" -eq 1 ]; then
    say "would run $GLEN_BIN --profile $PROFILE install-extension --json"
  else
    glen_run --profile "$PROFILE" install-extension --json
  fi
fi

if [ "$LOGIN" -eq 0 ]; then
  say "Owner action: run 'omp --profile glen-harness login cursor', then rerun"
fi

if [ "$CHECK" -eq 1 ]; then
  FAILED=0
  row() { printf '%-30s %s\n' "$1" "$2"; }
  row 'profile dir' "$AGENT_DIR"
  [ -d "$AGENT_DIR" ] || FAILED=1
  OMP_VERSION="$(profile_omp profile --version)"
  row 'omp version' "$OMP_VERSION"
  if [ "${OMP_VERSION##*/}" != 18.14.0 ]; then
    say 'WARN: expected omp 18.14.0' >&2
  fi
  row 'glen binary' "$GLEN_BIN"
  if GLEN_VERSION="$(glen_run --version)"; then
    row 'glen version' "$GLEN_VERSION"
  else
    row 'glen version' unavailable
    FAILED=1
  fi
  row model "$MODEL"
  # Indexes correspond to the fixed KEYS list above.
  row 'approvalMode (profile)' "${CURRENT[1]}"
  row 'approvalMode (overlay)' "${DESIRED[1]}"
  row 'fetch.enabled (overlay)' "${DESIRED[5]}"
  row 'restrictedFolders (overlay)' "${DESIRED[4]}"
  if [ -f "$MCP_TARGET" ] && jq -e '.mcpServers | has("mnemosyne")' "$MCP_TARGET" >/dev/null; then
    row 'MCP server present' yes
  else
    row 'MCP server present' no
    FAILED=1
  fi
  if [ -f "$SKILL_TARGET" ]; then row 'skill present' yes; else row 'skill present' no; FAILED=1; fi
  if ! cmp -s "$MCP_SOURCE" "$MCP_TARGET" || ! cmp -s "$SKILL_SOURCE" "$SKILL_TARGET"; then
    FAILED=1
  fi
  if DOCTOR="$(glen_run --profile "$PROFILE" doctor --json)"; then
    row 'extension status' "$(printf '%s' "$DOCTOR" | jq -r '.data.extension.state')"
    if ! printf '%s' "$DOCTOR" | jq -e '.data.extension.current == true' >/dev/null; then FAILED=1; fi
  else
    row 'extension status' unavailable
    FAILED=1
  fi
  if [ "$LOGIN" -eq 1 ]; then row 'Cursor login' confirmed; else row 'Cursor login' 'missing/not confirmed'; fi
  [ "$DIFFERENCES" -eq 0 ] || FAILED=1
  [ "$MODEL_OK" -eq 1 ] || FAILED=1
  [ "$FAILED" -eq 0 ] || exit 1
fi
[ "$MODEL_OK" -eq 1 ] || exit 1
if [ "$LOGIN" -eq 0 ]; then
  exit 2
fi
