#!/usr/bin/env bash
# Set up the isolated omp reviewer/debugger profile; reruns sync only differences.
# Usage: omp-profile-setup.sh [--model cursor/<id>] [--check] [--dry-run]
set -euo pipefail

PROFILE=glen-harness
DEFAULT_MODEL=cursor/gemini-3.1-pro
MODEL="$DEFAULT_MODEL"
CHECK=0
DRY_RUN=0
say() { printf '%s\n' "$*"; }
die() { say "ERROR: $*" >&2; exit 1; }

while [ "$#" -gt 0 ]; do
  case "$1" in
    --model)
      [ "$#" -ge 2 ] || die '--model needs cursor/<id>'
      MODEL="$2"
      shift 2
      ;;
    --check) CHECK=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --help | -h) sed -n '2,3s/^# //p' "$0"; exit 0 ;;
    *) die "Unknown arg: $1 (use --help)" ;;
  esac
done
case "$MODEL" in cursor/?*) ;; *) die '--model must be cursor/<id>' ;; esac

ENGINE_ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
MAIN="$(dirname "$(git -C "$ENGINE_ROOT" rev-parse --path-format=absolute --git-common-dir)")"
# PI_CONFIG_DIR is a directory name under HOME, including when it starts with /.
OMP_ROOT="$HOME/${PI_CONFIG_DIR:-.omp}"
OVERLAY="$ENGINE_ROOT/.claude/omp/glen-harness.yml"
MCP_SOURCE="$ENGINE_ROOT/.claude/omp/mcp.json"
SKILL_SOURCE="$ENGINE_ROOT/.claude/omp/skills/mnemosyne-glen/SKILL.md"
SKILL_MARKER='<!-- Managed by omp-profile-setup.sh: mnemosyne-glen -->'
for tool in omp jq shasum; do
  command -v "$tool" >/dev/null 2>&1 || die "Required tool missing: $tool"
done

fingerprint() {
  local agent="$OMP_ROOT/agent" file
  for file in config.yml mcp.json; do
    if [ -f "$agent/$file" ]; then
      printf '%s ' "$file"
      shasum -a 256 < "$agent/$file" || return 1
    else
      say "$file absent"
    fi
  done
  say extensions
  if [ -d "$agent/extensions" ]; then
    find "$agent/extensions" -mindepth 1 -maxdepth 1 -print | LC_ALL=C sort || return 1
  else
    say absent
  fi
  say sessions
  if [ -d "$agent/sessions" ]; then
    if [ "$(uname -s)" = Darwin ]; then
      find "$agent/sessions" -type f -exec stat -f '%m' {} + || return 1
    else
      find "$agent/sessions" -type f -exec stat -c '%Y' {} + || return 1
    fi | awk 'BEGIN { count=0; newest=0 } { count++; if ($1>newest) newest=$1 } END { print count, newest }' || return 1
  else
    say '0 0'
  fi
}

BEFORE="$(fingerprint)"
TMPROOT="$(mktemp -d)"
finish() {
  local status="$?" after
  trap - EXIT
  if after="$(fingerprint)" && [ "$BEFORE" = "$after" ]; then
    [ "$CHECK" -eq 0 ] || printf '%-30s %s\n' fingerprint-unchanged yes
  else
    printf '%-30s %s\n' fingerprint-unchanged no >&2
    say 'ERROR: default-profile fingerprint changed (or could not be read); inspect it manually.' >&2
    status=1
  fi
  rm -rf "$TMPROOT"
  exit "$status"
}
trap finish EXIT

# Each call gets a new project-free cwd. Never inherit the caller's overlay or
# active profile; the unqualified config path must refer to the default profile.
omp_run() {
  local layer="$1"
  shift
  (
    cd "$(mktemp -d "$TMPROOT/omp.XXXXXX")" || exit 1
    if [ "$layer" = overlay ]; then
      env -u CLAUDE_CONFIG_DIR -u PI_PY -u PI_JS -u PI_CONFIG_FILES \
        -u OMP_PROFILE -u PI_PROFILE PI_CONFIG_FILES="$OVERLAY" omp "$@"
    else
      env -u CLAUDE_CONFIG_DIR -u PI_PY -u PI_JS -u PI_CONFIG_FILES \
        -u OMP_PROFILE -u PI_PROFILE omp "$@"
    fi
  )
}
profile_omp() {
  local layer="$1"
  shift
  omp_run "$layer" --profile "$PROFILE" "$@"
}

AGENT_DIR="$(profile_omp profile config path)"
DEFAULT_AGENT_DIR="$(omp_run profile config path)"
[ -n "$AGENT_DIR" ] || die 'omp returned an empty profile directory'
[ "$AGENT_DIR" != "$DEFAULT_AGENT_DIR" ] || die 'Harness profile resolves to the default profile!'
case "$AGENT_DIR" in
  /*/profiles/glen-harness/agent) ;;
  *) die "Unsafe harness profile directory: $AGENT_DIR" ;;
esac
MCP_TARGET="$AGENT_DIR/mcp.json"
SKILL_TARGET="$AGENT_DIR/skills/mnemosyne-glen/SKILL.md"

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

KEYS=(modelRoles.default tools.approvalMode tools.approval tools.xdev
  tools.restrictedFolders fetch.enabled web_search.enabled bash.enabled eval.py
  eval.js browser.enabled lsp.enabled mcp.enableProjectConfig mcp.startupTimeoutMs)
CURRENT=() DESIRED=()
DIFFERENCES=0
for key in "${KEYS[@]}"; do
  current="$(profile_omp profile config get "$key" --json | jq -cS .value)"
  if [ "$key" = modelRoles.default ]; then
    desired="$(jq -cn --arg model "$MODEL" '$model')"
  else
    desired="$(profile_omp overlay config get "$key" --json | jq -cS .value)"
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
      profile_omp profile config set "$key" "$desired" >/dev/null
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
      -u OMP_PROFILE -u PI_PROFILE "$GLEN_BIN" "$@"
  )
}
if [ "$CHECK" -eq 0 ]; then
  if [ "$DRY_RUN" -eq 1 ]; then
    say "would run $GLEN_BIN --profile $PROFILE install-extension --json"
  else
    glen_run --profile "$PROFILE" install-extension --json
  fi
fi

# Primary login signal, pending a live owner login. If unreliable, an alternate
# is `omp --profile glen-harness usage accounts --json` containing a cursor entry.
MODELS="$(profile_omp profile models cursor --json)"
MODEL_COUNT="$(printf '%s' "$MODELS" | jq -er '.models | length')"
LOGIN=1
[ "$MODEL_COUNT" -gt 0 ] || LOGIN=0
MODEL_OK=1
if [ "$LOGIN" -eq 1 ] && ! printf '%s' "$MODELS" | jq -e --arg model "$MODEL" \
  'any(.models[]; (.selector // (.provider + "/" + .id)) == $model)' >/dev/null; then
  MODEL_OK=0
  say "ERROR: model $MODEL is not listed. Available ids:" >&2
  printf '%s' "$MODELS" | jq -r '.models[] | .selector // (.provider + "/" + .id)' >&2
fi
if [ "$LOGIN" -eq 0 ]; then
  say "Owner action: run 'omp --profile glen-harness login cursor', then rerun"
fi

if [ "$CHECK" -eq 1 ]; then
  FAILED=0
  row() { printf '%-30s %s\n' "$1" "$2"; }
  row 'profile dir' "$AGENT_DIR"
  [ -d "$AGENT_DIR" ] || FAILED=1
  OMP_VERSION="$(omp_run profile --version)"
  row 'omp version' "$OMP_VERSION"
  if [ "$OMP_VERSION" != 18.14.0 ]; then
    say 'WARN: expected omp 18.14.0' >&2
    FAILED=1
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
