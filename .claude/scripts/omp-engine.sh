#!/usr/bin/env bash
# Isolated third review/debug engine. Policy/envcheck are internal smoke helpers.
set -euo pipefail
umask 077
# A signal trap can run while dispatch_env's stdout is redirected to response.raw.
# Keep the public result stream available even inside that redirected function.
exec 3>&1

MODE="${1:-}"
[ "$#" -eq 0 ] || shift
PROFILE=glen-harness
START="$(date +%s)"
STATUS=failed REASON='engine command failed' EXIT_CODE=4
MODEL='' THINKING_LEVEL='' PROVIDER='' SOURCE_FINGERPRINT='' POLICY_SOURCE='' LAUNCHER_SOURCE='' LAUNCHER_BIN='' HEAD='' MERGE_BASE='' SNAPSHOT_SHA256=''
RUN_DIR='' RUN_GLEN_HOME='' POLICY_PATH='' JOB='' SESSION_HANDLE='' SESSION_PATH=''
OUT='' PROBE_DIR='' ENVELOPE='' TREE='' ROOT='' MOVED='[]' REPLIES=0 KEEP=0
OUTPUT_READY=0 JOB_ACTIVE=0 JOB_TIMEOUT_SECONDS=0
USAGE='{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost_total":0}'

log() { printf 'omp-engine: %s\n' "$*" >&2; }
stop() { STATUS="$1"; REASON="$2"; EXIT_CODE="$3"; exit "$EXIT_CODE"; }
# shellcheck disable=SC2329 # Registered below for INT/TERM.
interrupted() {
  [ -z "$JOB" ] || stop timeout 'dispatch interrupted; job may still be running' 5
  stop failed 'dispatch interrupted before any job handle was obtained' 4
}
usage() {
  log 'usage: omp-engine.sh probe | review [--repo DIR] (--base REF | --uncommitted) --out FILE [--scope TEXT] [--deadline 15m] [--keep]'
  log '       omp-engine.sh debug [--repo DIR] --brief FILE --out FILE [--base REF] [--deadline 20m] [--keep]'
  log '       omp-engine.sh snapshot SRC DEST | policy SRC | envcheck -- CMD...'
  stop failed 'usage error' 2
}

# The single environment boundary for send, wait, and the envcheck smoke helper.
# Mnemosyne credentials pass through untouched; never log environment values.
profile_agent_dir() {
  local dir double='//' single='/'
  dir="$(cd "$HOME" && pwd -P)/${PI_CONFIG_DIR:-.omp}/profiles/$PROFILE/agent"
  # PI_CONFIG_DIR is joined under HOME even with a leading slash (setup
  # normalizes it the same way); collapse the doubled separator so path
  # comparisons see one spelling (fix cycle 6, omp item-5 finding).
  while [[ "$dir" == *//* ]]; do dir="${dir//$double/$single}"; done
  printf '%s\n' "$dir"
}
# A fixed, per-TMPDIR cache outside any per-dispatch run directory. The sandbox
# profile grants file-write* only on OMP_SANDBOX_RUN_ROOT (the per-dispatch run
# directory) and specific profile paths; the launcher must never live there,
# or the sandboxed child could overwrite its own policy enforcement.
launcher_dir_for() {
  local dir
  dir="${TMPDIR:-/tmp}"
  dir="$(cd "$dir" && pwd -P)" || return 1
  printf '%s/glen-omp-engine-launcher\n' "$dir"
}
dispatch_env() {
  local engine_root agent_dir real_omp
  engine_root="${ENGINE_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)}"
  resolve_launcher "$engine_root" "$(launcher_dir_for)" || { log "$REASON"; stop failed "$REASON" 4; }
  agent_dir="$(profile_agent_dir)"
  real_omp="$(command -v omp)" || stop failed 'omp not on PATH' 4
  case "$real_omp" in /*) ;; *) real_omp="$PWD/$real_omp" ;; esac
  # XDG_*_HOME would move omp's and glen's sessions and leases away from the
  # ~/.omp layout the sandbox grants (fix cycle 6, omp item-5 finding).
  env -u CLAUDE_CONFIG_DIR -u COPILOT_CUSTOM_INSTRUCTIONS_DIRS -u PI_PY -u PI_JS -u PI_CODING_AGENT_DIR \
    -u OMP_CODING_AGENT_DIR -u OMP_PROFILE -u PI_PROFILE \
    -u XDG_STATE_HOME -u XDG_DATA_HOME -u XDG_CACHE_HOME -u XDG_CONFIG_HOME \
    -u GLEN_OMP_BIN -u OMP_SANDBOX_RUN_ROOT -u OMP_REAL_BIN -u OMP_PROFILE_AGENT_DIR \
    GLEN_OMP_BIN="$LAUNCHER_BIN" OMP_REAL_BIN="$real_omp" \
    OMP_SANDBOX_RUN_ROOT="$RUN_DIR" OMP_PROFILE_AGENT_DIR="$agent_dir" \
    GLEN_HOME="$RUN_GLEN_HOME" PI_CONFIG_FILES="$POLICY_PATH" \
    GLEN_JOB_TIMEOUT="${JOB_TIMEOUT_SECONDS}s" "$@" 3>&-
}

# Refuses if $1's permission bits allow group or other write, or if it is not
# owned by the current user. Used for the launcher cache directory and file.
check_no_group_other_write() {
  local path="$1" owner permissions
  if [ "$(uname -s)" = Darwin ]; then
    owner="$(stat -f '%u' "$path")" && permissions="$(stat -f '%Lp' "$path")" || return 1
  else
    owner="$(stat -c '%u' "$path")" && permissions="$(stat -c '%a' "$path")" || return 1
  fi
  [ "$owner" = "$(id -u)" ] || return 1
  (( (8#$permissions & 0022) == 0 ))
}
# Fix cycle 7 (Codex P1): no component of the glen-harness profile path, from
# the omp root down through profiles/glen-harness/agent and the runtime
# children the sandbox can write, may be a symlink. A relocated profile (for
# example profiles/glen-harness -> /elsewhere) would let the containment
# checks below compare against one spelling while the sandbox grants writes
# to another, so it is refused with its path, never followed. With no
# symlinks in the chain, the lexical and physical profile paths agree.
profile_symlink_guard() {
  local home_real omp_root part path profile_root
  local -a parts=()
  home_real="$(cd "$HOME" && pwd -P)" || { REASON='could not resolve HOME'; return 1; }
  omp_root="$home_real"
  IFS=/ read -r -a parts <<< "${PI_CONFIG_DIR:-.omp}"
  if [ "${#parts[@]}" -gt 0 ]; then
    for part in "${parts[@]}"; do
      case "$part" in
        '' | .) continue ;;
        ..) REASON='PI_CONFIG_DIR must not contain a .. component'; return 1 ;;
      esac
      omp_root="$omp_root/$part"
      [ ! -L "$omp_root" ] || { REASON="refusing symlinked $PROFILE profile path: $omp_root"; return 1; }
    done
  fi
  profile_root="$omp_root/profiles/$PROFILE"
  for path in "$omp_root/profiles" "$profile_root" "$profile_root/agent" "$profile_root/agent/sessions" \
    "$profile_root/logs" "$profile_root/cache" "$profile_root/run" "$profile_root/run/daemons" \
    "$omp_root/run" "$omp_root/run/session-owners"; do
    [ ! -L "$path" ] || { REASON="refusing symlinked $PROFILE profile path: $path"; return 1; }
  done
}
# The physical spelling of a directory that need not exist yet: the nearest
# existing ancestor resolved with pwd -P, plus the missing suffix. Fails on a
# non-directory or dangling-symlink ancestor and on . or .. in the suffix.
physical_spelling() {
  local path="$1" suffix='' name base
  while [ "${path%/}" != "$path" ] && [ "$path" != / ]; do path="${path%/}"; done
  while [ ! -d "$path" ]; do
    [ ! -e "$path" ] && [ ! -L "$path" ] || return 1
    name="${path##*/}"
    case "$name" in '' | . | ..) return 1 ;; esac
    suffix="/$name$suffix"
    case "$path" in */*) path="${path%/*}"; [ -n "$path" ] || path=/ ;; *) path=. ;; esac
  done
  base="$(cd "$path" && pwd -P)" || return 1
  [ "$base" != / ] || base=''
  printf '%s%s\n' "$base" "$suffix"
}
# Physical containment (fix cycle 5, decision 1): DEST is already resolved, so
# a cache dir reached only through a symlink into a sandbox-writable tree is
# checked at its real, writable location. A lexical check could not.
launcher_dest_refusal() {
  local dest="$1" home_real writable writable_real
  case "$dest/" in
    "$(profile_agent_dir)"/*) REASON='sandbox launcher must not resolve under the profile agent directory'; return 1 ;;
  esac
  case "$dest" in
    */glen-omp-engine/run-*)
      REASON='sandbox launcher must not resolve inside a per-dispatch run directory'
      return 1
      ;;
  esac
  # Fix cycle 6 (Claude MED-1 / Codex P1): compare the resolved directory with
  # every root the sandboxed child can write to, also resolved: the whole omp
  # home (profile logs/cache/run, sessions, ~/.omp/run), every engine run and
  # probe root, and any active run root.
  home_real="$(cd "$HOME" && pwd -P)" || { REASON='could not resolve HOME'; return 1; }
  for writable in "$home_real/.omp" "$home_real/${PI_CONFIG_DIR:-.omp}" "${RUN_DIR:-}" "${OMP_SANDBOX_RUN_ROOT:-}"; do
    [ -n "$writable" ] && [ -d "$writable" ] || continue
    writable_real="$(cd "$writable" && pwd -P)" || { REASON='could not resolve a sandbox-writable root'; return 1; }
    case "$dest/" in
      "$writable_real"/*)
        REASON="sandbox launcher must not resolve under sandbox-writable $writable_real"
        return 1
        ;;
    esac
  done
  case "$dest/" in
    */glen-omp-engine/*)
      REASON='sandbox launcher must not resolve inside the engine run root'
      return 1
      ;;
  esac
}
# The one platform/source decision, shared by dispatch, probes, and setup's
# internal `launcher SRC DIR` helper. DEST must never be a directory the
# sandboxed child can write to: refuse any destination that is, or sits
# inside, a per-dispatch run directory (the "run-*" naming resolve_root uses),
# and re-verify owner/mode on every dispatch, not just at creation.
resolve_launcher() {
  local src="$1" dest="$2" launcher tool platform
  platform="$(uname -s)"
  # The omp engine is unavailable on Linux (probe_checks gates this earlier);
  # this is a second, independent refusal so resolve_launcher never selects
  # the bwrap launcher even if called directly.
  [ "$platform" = Darwin ] || { REASON='unavailable: linux abstract-socket isolation not supported'; return 1; }
  launcher=omp-sandboxed.sh; tool=sandbox-exec
  if ! command -v "$tool" >/dev/null 2>&1; then
    REASON="$tool not on PATH; the omp sandbox requires it on macOS"
    return 1
  fi
  profile_symlink_guard || return 1
  case "$dest" in
    */glen-omp-engine/run-*)
      REASON='sandbox launcher must not be placed inside a per-dispatch run directory'
      return 1
      ;;
  esac
  if [ -n "${RUN_DIR:-}" ]; then
    case "$dest/" in
      "$RUN_DIR"/*) REASON='sandbox launcher must not be placed inside the active run directory'; return 1 ;;
    esac
    [ "$dest" != "$RUN_DIR" ] || { REASON='sandbox launcher must not be placed inside the active run directory'; return 1; }
  fi
  if [ -n "${OMP_SANDBOX_RUN_ROOT:-}" ]; then
    case "$dest/" in
      "$OMP_SANDBOX_RUN_ROOT"/*) REASON='sandbox launcher must not be placed inside an active sandbox run root'; return 1 ;;
    esac
    [ "$dest" != "$OMP_SANDBOX_RUN_ROOT" ] || { REASON='sandbox launcher must not be placed inside an active sandbox run root'; return 1; }
  fi
  # Run every refusal on the planned physical spelling before creating
  # anything (fix cycle 7, Claude LOW-5), then again on the created directory.
  local planned
  planned="$(physical_spelling "$dest")" || { REASON='could not resolve sandbox launcher directory'; return 1; }
  launcher_dest_refusal "$planned" || return 1
  mkdir -p "$dest" || { REASON='could not create sandbox launcher directory'; return 1; }
  dest="$(cd "$dest" && pwd -P)" || { REASON='could not resolve sandbox launcher directory'; return 1; }
  launcher_dest_refusal "$dest" || return 1
  chmod 0700 "$dest" || { REASON='could not secure sandbox launcher directory'; return 1; }
  check_no_group_other_write "$dest" || {
    REASON="unsafe sandbox launcher directory: $dest is not exclusively owned and writable by the current user"
    return 1
  }
  # Fetch into a scratch file, then adopt a content-addressed final name so
  # repeated calls reuse the same file instead of piling up mktemp siblings
  # (LOW-3), and remove any stale sibling this cache dir owns under the same
  # launcher basename. Recheck ownership/mode and source on every call; never
  # select an inherited GLEN_OMP_BIN/OMP_REAL_BIN.
  local fetch_tmp hash final stale
  fetch_tmp="$(mktemp "$dest/.$launcher.fetch.XXXXXX")" || {
    REASON='could not create sandbox launcher'
    return 1
  }
  if git -C "$src" show "main:.claude/omp/$launcher" > "$fetch_tmp" 2>/dev/null; then
    LAUNCHER_SOURCE=main
  else
    if ! cp "$src/.claude/omp/$launcher" "$fetch_tmp" 2>/dev/null; then
      rm -f "$fetch_tmp"
      REASON='sandbox launcher unavailable on main and in worktree'
      return 1
    fi
    LAUNCHER_SOURCE=worktree
    log 'warning: main launcher unavailable; using the worktree launcher'
  fi
  chmod 0500 "$fetch_tmp" || { rm -f "$fetch_tmp"; REASON='could not secure sandbox launcher file'; return 1; }
  if [ "$(uname -s)" = Darwin ]; then
    hash="$(shasum -a 256 "$fetch_tmp" | awk '{print $1}')"
  else
    hash="$(sha256sum "$fetch_tmp" | awk '{print $1}')"
  fi
  if [ -z "$hash" ]; then
    rm -f "$fetch_tmp"
    REASON='could not hash sandbox launcher'
    return 1
  fi
  final="$dest/$launcher.${hash:0:16}"
  # Reuse only a regular file with exactly the fetched bytes (fix cycle 6,
  # MED-1); replace anything else, including a symlink, under that name.
  if [ -f "$final" ] && [ ! -L "$final" ] && cmp -s "$fetch_tmp" "$final"; then
    rm -f "$fetch_tmp"
    touch "$final" 2>/dev/null || true
  else
    if [ -e "$final" ] || [ -L "$final" ]; then
      rm -f "$final" 2>/dev/null || { rm -f "$fetch_tmp"; REASON='could not replace a mismatched sandbox launcher'; return 1; }
    fi
    if ! mv "$fetch_tmp" "$final"; then
      rm -f "$fetch_tmp"
      REASON='could not install sandbox launcher'
      return 1
    fi
  fi
  [ -f "$final" ] && [ ! -L "$final" ] || { REASON='sandbox launcher is not a regular file'; return 1; }
  check_no_group_other_write "$final" || {
    REASON='unsafe sandbox launcher file permissions after creation'
    return 1
  }
  LAUNCHER_BIN="$final"
  # Another worktree's engine may be about to exec its own version from this
  # cache (Claude LOW-4): prune only launchers and abandoned fetch files that
  # nothing has used for a day. Reuse refreshes the mtime above.
  for stale in "$dest/$launcher".* "$dest/.$launcher.fetch".*; do
    [ -e "$stale" ] || [ -L "$stale" ] || continue
    [ "$stale" != "$final" ] || continue
    [ -n "$(find "$stale" -prune -mtime +0 2>/dev/null)" ] || continue
    rm -f "$stale"
  done
}

# SRC is an engine checkout, not the tree being reviewed.
policy() {
  local src="$1" dest="$2"
  if git -C "$src" show main:.claude/omp/glen-harness.yml > "$dest" 2>/dev/null; then
    POLICY_SOURCE=main
  else
    cp "$src/.claude/omp/glen-harness.yml" "$dest"
    POLICY_SOURCE=worktree
    log 'warning: main policy unavailable; using the worktree policy'
  fi
}

# Internal helpers deliberately expose only their requested command/path/YAML.
case "$MODE" in
  launcher)
    case "$#" in
      1) resolve_launcher "$1" "$(launcher_dir_for)" || { log "$REASON"; exit 4; } ;;
      2) resolve_launcher "$1" "$2" || { log "$REASON"; exit 4; } ;;
      *) log 'usage: launcher SRC [DIR]'; exit 2 ;;
    esac
    printf '%s\n' "$LAUNCHER_BIN"
    exit 0
    ;;
  envcheck)
    if [ "${1:-}" != -- ] || [ "$#" -lt 2 ]; then
      printf '%s\n' '{"v":1,"engine":"omp","mode":"envcheck","status":"failed","reason":"usage error"}'
      exit 2
    fi
    shift
    RUN_DIR="$(mktemp -d)"
    trap 'rm -rf "$RUN_DIR"' EXIT
    dispatch_env "$@"
    exit "$?"
    ;;
  policy)
    if [ "$#" -ne 1 ]; then
      printf '%s\n' '{"v":1,"engine":"omp","mode":"policy","status":"failed","reason":"usage error"}'
      exit 2
    fi
    policy_tmp="$(mktemp)"
    trap 'rm -f "$policy_tmp"' EXIT
    policy "$1" "$policy_tmp"
    printf '%s\n' "$POLICY_SOURCE"
    cat "$policy_tmp"
    exit 0
    ;;
esac
if ! command -v jq >/dev/null 2>&1; then
  printf '%s\n' '{"v":1,"engine":"omp","status":"failed","reason":"jq not on PATH"}'
  exit 4
fi

result_json() {
  local elapsed
  elapsed=$(( ($(date +%s) - START) * 1000 ))
  jq -cn --arg mode "$MODE" --arg status "$STATUS" --arg reason "$REASON" \
    --arg model "$MODEL" --arg provider "$PROVIDER" --arg policy_source "$POLICY_SOURCE" \
    --arg launcher_source "$LAUNCHER_SOURCE" \
    --arg head "$HEAD" --arg merge_base "$MERGE_BASE" --arg snapshot_sha256 "$SNAPSHOT_SHA256" \
    --arg glen_home "$RUN_GLEN_HOME" --arg job "$JOB" --arg handle "$SESSION_HANDLE" \
    --arg path "$SESSION_PATH" --arg out "$OUT" --arg run_dir "$RUN_DIR" \
    --argjson elapsed_ms "$elapsed" --argjson usage "$USAGE" --argjson replies "$REPLIES" \
    --argjson moved "$MOVED" --arg source_fingerprint "$SOURCE_FINGERPRINT" '
    def nullable: if . == "" then null else . end;
    if $mode == "snapshot" then
      {v:1,engine:"omp",mode:$mode,status:$status,reason:($reason|nullable),head:($head|nullable),
       snapshot_sha256:($snapshot_sha256|nullable),moved:$moved,source_fingerprint:($source_fingerprint|nullable)}
    else
      {v:1,engine:"omp",mode:$mode,status:$status,reason:($reason|nullable),profile:"glen-harness",
       model:($model|nullable),provider:($provider|nullable),policy_source:($policy_source|nullable),
       launcher_source:($launcher_source|nullable),
       head:($head|nullable),merge_base:($merge_base|nullable),snapshot_sha256:($snapshot_sha256|nullable),
       glen_home:($glen_home|nullable),job:($job|nullable),session:{handle:($handle|nullable),path:($path|nullable)},
       elapsed_ms:$elapsed_ms,usage:$usage,replies:$replies,out:($out|nullable),run_dir:($run_dir|nullable)}
    end'
}
save_result() {
  local tmp
  tmp="$(mktemp "${OUT}.json.XXXXXX")"
  result_json > "$tmp"
  mv "$tmp" "$OUT.json"
}

# shellcheck disable=SC2329 # Called by the EXIT handler.
capture() {
  local meta last_line transcript_model
  [ -n "$ENVELOPE" ] && [ -s "$ENVELOPE" ] || return 0
  jq -jr '.data.assistant_text // ""' "$ENVELOPE" > "$OUT" || return 1
  REPLIES="$(jq -r '(.data.replies // []) | length' "$ENVELOPE")" || return 1
  if [ -n "$SESSION_PATH" ] && [ -f "$SESSION_PATH" ]; then
    meta="$(jq -sc '
      [.[] | select(.type == "message" and .message.role == "assistant") | .message] as $a |
      {model:($a[-1].model // ""),provider:($a[-1].provider // ""),usage:
        (reduce $a[] as $m ({input:0,output:0,cacheRead:0,cacheWrite:0,totalTokens:0,cost_total:0};
          .input += ($m.usage.input // 0) | .output += ($m.usage.output // 0) |
          .cacheRead += ($m.usage.cacheRead // 0) | .cacheWrite += ($m.usage.cacheWrite // 0) |
          .totalTokens += ($m.usage.totalTokens // 0) | .cost_total += ($m.usage.cost.total // 0)))}
      ' "$SESSION_PATH")" || return 1
    # The transcript records a bare model id; report it as provider/id, the
    # same form probe and setup use (Step 11 acceptance, item 2).
    # Keep the probed model when the transcript has no assistant message yet
    # (fix cycle 6, omp item-5 finding).
    transcript_model="$(printf '%s' "$meta" | jq -r '
      if .model != "" and .provider != "" and (.model | contains("/") | not)
      then .provider + "/" + .model else .model end')"
    [ -z "$transcript_model" ] || MODEL="$transcript_model"
    PROVIDER="$(printf '%s' "$meta" | jq -r .provider)"
    USAGE="$(printf '%s' "$meta" | jq -c .usage)"
  fi
  # Preserve timeout status: its tree must stay available to a possibly live job.
  if [ "$STATUS" != timeout ] && [ -n "$PROVIDER" ] && [ "$PROVIDER" != cursor ]; then
    STATUS=failed REASON="ran on $PROVIDER, not cursor" EXIT_CODE=4
  elif [ "$STATUS" = ok ] && [ -z "$PROVIDER" ]; then
    STATUS=failed REASON='ran on unknown, not cursor' EXIT_CODE=4
  fi
  if [ "$STATUS" = ok ] && ! grep -q '[^[:space:]]' "$OUT"; then
    STATUS=failed REASON='omp returned no final answer' EXIT_CODE=4
  fi
  if [ "$STATUS" = ok ]; then
    if [ "$MODE" = review ]; then
      last_line="$(awk 'NF {last=$0} END {print last}' "$OUT")"
      if ! grep -Eq '^## omp review[[:space:]]*$' "$OUT" || \
        ! printf '%s\n' "$last_line" | grep -Eq '^Mnemosyne:'; then
        STATUS=failed REASON='malformed output'
      fi
    elif ! grep -Eq '^#{1,6}[[:space:]]+Root Cause[[:space:]]*$' "$OUT"; then
      STATUS=failed REASON='malformed output'
    fi
    # A content-shape failure has no vote, but does not fail a completed dispatch.
  fi
}

# shellcheck disable=SC2329 # Registered below as the EXIT handler.
finish() {
  local shell_status="$?"
  trap - EXIT
  set +e
  if [ "$shell_status" -ne 0 ] && [ "$JOB_ACTIVE" -eq 1 ] && [ -n "$JOB" ] && [ "$STATUS" != timeout ]; then
    STATUS=timeout REASON='dispatch interrupted; job may still be running' EXIT_CODE=5
  fi
  if [ "$OUTPUT_READY" -eq 1 ]; then
    : > "$OUT"
    if ! capture; then
      if [ "$STATUS" = ok ]; then STATUS=failed REASON='could not capture session usage' EXIT_CODE=4; fi
    fi
    save_result || { log 'could not write result JSON'; STATUS=failed REASON='could not write result JSON' EXIT_CODE=4; }
  fi
  if [ -n "$TREE" ] && [ "$STATUS" != timeout ] && [ "$KEEP" -eq 0 ]; then
    rm -rf "$TREE"
  fi
  [ -z "$PROBE_DIR" ] || rm -rf "$PROBE_DIR"
  result_json >&3
  exit "$EXIT_CODE"
}
trap finish EXIT
trap interrupted INT TERM

# The disabled probe must not pay for repository/tool discovery in a fresh TMPDIR.
# Keep its JSON identical to other skips.
if [ "$MODE" = probe ] && [ "$#" -eq 0 ] && [ "${GLEN_OMP_ENGINE:-}" = off ]; then
  stop skipped 'disabled by GLEN_OMP_ENGINE=off' 3
fi
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
ENGINE_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"
SCRIPT="$SCRIPT_DIR/omp-engine.sh"

under() { case "$1/" in "$2/"*) return 0 ;; *) return 1 ;; esac; }
prepare_root() {
  local parent home metadata owner permissions
  # Callers retain this diagnostic when preparation fails, including stat errors.
  REASON='unsafe run root: could not resolve or inspect directory'
  parent="$(cd "${TMPDIR:-/tmp}" && pwd -P)" || return 1
  home="$(cd "$HOME" && pwd -P)" || return 1
  ROOT="$parent/glen-omp-engine"
  if [ -d "$ROOT" ]; then ROOT="$(cd "$ROOT" && pwd -P)" || return 1; fi
  # shellcheck disable=SC2016 # Diagnostic deliberately names HOME literally.
  if under "$ROOT" "$home"; then REASON='run root under $HOME; set TMPDIR outside $HOME'; return 1; fi
  mkdir -p "$ROOT" || return 1
  ROOT="$(cd "$ROOT" && pwd -P)" || return 1
  if [ "$(uname -s)" = Darwin ]; then
    metadata="$(stat -f '%u %Lp' "$ROOT")" || return 1
  else
    metadata="$(stat -c '%u %a' "$ROOT")" || return 1
  fi
  read -r owner permissions <<< "$metadata"
  if [ "$owner" != "$(id -u)" ]; then
    REASON="unsafe run root: $ROOT is not owned by the current user"
    return 1
  fi
  if (( (8#$permissions & 0022) != 0 )); then
    REASON="unsafe run root: $ROOT is group- or world-writable"
    return 1
  fi
}

# omp 18.14.0 discovery/agents-md.ts:20-21 and claude-md.ts:20-21 delegate
# to discovery/helpers.ts:723-783, which walks parents (without a Git boundary
# for these git-free snapshots). Inspect the physical chain all the way to /,
# including ROOT's parent; shared sticky directories get no permission exception.
validate_snapshot_ancestors() {
  local ancestor context permissions matches
  ancestor="$(cd "$(dirname "$1")" && pwd -P)" || stop skipped 'unsafe snapshot ancestor: could not resolve parent' 3
  while :; do
    # Glob only this directory's own entry names, instead of a find traversal:
    # a sibling vanishing mid-scan must never turn into a false "could not
    # inspect" failure (Claude MED-002). nullglob makes a non-match expand to
    # nothing rather than the literal pattern. Bracket-class patterns are real
    # glob metacharacters, so every AGENTS.md/CLAUDE.md case variant is caught
    # on a case-SENSITIVE filesystem too (fix cycle 5, decision 6); plain
    # nocaseglob alone has no effect on a pattern with no metacharacters at
    # all, which only worked here by accident on a case-insensitive one.
    shopt -s nullglob
    matches=(
      "$ancestor"/[Aa][Gg][Ee][Nn][Tt][Ss].[Mm][Dd]
      "$ancestor"/[Cc][Ll][Aa][Uu][Dd][Ee].[Mm][Dd]
    )
    shopt -u nullglob
    # Bash 3.2 (macOS's default) treats "${arr[@]}" on a zero-element array
    # as unbound under set -u; guard the count before iterating.
    if [ "${#matches[@]}" -gt 0 ]; then
      for context in "${matches[@]}"; do
        [ -f "$context" ] || [ -L "$context" ] || continue
        stop skipped "unsafe snapshot ancestor: context file $context" 3
      done
    fi
    if [ "$(uname -s)" = Darwin ]; then
      permissions="$(stat -f '%Lp' "$ancestor")" || stop skipped "unsafe snapshot ancestor: could not stat $ancestor" 3
    else
      permissions="$(stat -c '%a' "$ancestor")" || stop skipped "unsafe snapshot ancestor: could not stat $ancestor" 3
    fi
    if (( (8#$permissions & 0022) != 0 )); then
      stop skipped "unsafe snapshot ancestor: $ancestor is group- or world-writable" 3
    fi
    [ "$ancestor" != / ] || break
    ancestor="$(dirname "$ancestor")"
  done
}

resolve_glen() {
  local common main
  if [ -n "${GLEN_BIN:-}" ]; then
    GLEN_BIN="$(command -v "$GLEN_BIN" 2>/dev/null)" || return 1
  elif command -v glen >/dev/null 2>&1; then
    GLEN_BIN="$(command -v glen)"
  else
    common="$(git -C "$ENGINE_ROOT" rev-parse --path-format=absolute --git-common-dir)" || return 1
    main="$(cd "$common/.." && pwd -P)" || return 1
    GLEN_BIN="$main/target/release/glen"
  fi
  [ -x "$GLEN_BIN" ] || return 1
  case "$GLEN_BIN" in /*) ;; *) GLEN_BIN="$PWD/$GLEN_BIN" ;; esac
}
bounded_omp() {
  local seconds="$1" cwd real_omp agent_dir
  shift
  cwd="$(mktemp -d "$PROBE_DIR/call.XXXXXX")" || return 1
  resolve_launcher "$ENGINE_ROOT" "$(launcher_dir_for)" || { log "$REASON"; return 1; }
  real_omp="$(command -v omp)" || return 1
  case "$real_omp" in /*) ;; *) real_omp="$PWD/$real_omp" ;; esac
  agent_dir="$(profile_agent_dir)"
  (
    cd "$cwd" || exit 1
    if command -v timeout >/dev/null 2>&1; then
      env -u CLAUDE_CONFIG_DIR -u COPILOT_CUSTOM_INSTRUCTIONS_DIRS -u PI_PY -u PI_JS -u PI_CONFIG_FILES \
        -u PI_CODING_AGENT_DIR -u OMP_CODING_AGENT_DIR -u OMP_PROFILE -u PI_PROFILE \
        -u XDG_STATE_HOME -u XDG_DATA_HOME -u XDG_CACHE_HOME -u XDG_CONFIG_HOME \
        OMP_REAL_BIN="$real_omp" OMP_SANDBOX_RUN_ROOT="$cwd" OMP_PROFILE_AGENT_DIR="$agent_dir" \
        timeout "$seconds" "$LAUNCHER_BIN" --profile "$PROFILE" "$@" 3>&-
    else
      env -u CLAUDE_CONFIG_DIR -u COPILOT_CUSTOM_INSTRUCTIONS_DIRS -u PI_PY -u PI_JS -u PI_CONFIG_FILES \
        -u PI_CODING_AGENT_DIR -u OMP_CODING_AGENT_DIR -u OMP_PROFILE -u PI_PROFILE \
        -u XDG_STATE_HOME -u XDG_DATA_HOME -u XDG_CACHE_HOME -u XDG_CONFIG_HOME \
        OMP_REAL_BIN="$real_omp" OMP_SANDBOX_RUN_ROOT="$cwd" OMP_PROFILE_AGENT_DIR="$agent_dir" \
        perl -e 'alarm shift @ARGV; exec @ARGV' "$seconds" "$LAUNCHER_BIN" --profile "$PROFILE" "$@" 3>&-
    fi
  )
}
probe_checks() {
  local models value rc
  [ "${GLEN_OMP_ENGINE:-}" != off ] || stop skipped 'disabled by GLEN_OMP_ENGINE=off' 3
  # Orchestrator decision (fix cycle 4): the omp engine is unavailable on
  # Linux in v0.1.0. bwrap cannot isolate abstract Unix sockets while still
  # letting the sandboxed process reach Cursor over the network, so there is
  # no safe bwrap policy today. omp-sandboxed-linux.sh stays in the repo for
  # a later Landlock-based version, but dispatch never calls it; this check
  # runs before resolve_launcher, regardless of whether bwrap happens to be
  # on PATH.
  if [ "$(uname -s)" != Darwin ]; then
    stop skipped 'unavailable: linux abstract-socket isolation not supported' 3
  fi
  resolve_glen || stop skipped 'no glen binary; run .claude/scripts/omp-profile-setup.sh' 3
  command -v omp >/dev/null 2>&1 || stop skipped 'omp not on PATH' 3
  command -v sandbox-exec >/dev/null 2>&1 || stop skipped 'sandbox-exec not on PATH; the omp sandbox requires it on macOS' 3
  [ -f "$(profile_agent_dir)/config.yml" ] || \
    stop skipped 'glen-harness profile is not set up; run .claude/scripts/omp-profile-setup.sh' 3
  prepare_root || stop skipped "$REASON" 3
  # Probe cwd is also below ROOT; reject unsafe discovery ancestry before omp.
  validate_snapshot_ancestors "$ROOT/.probe"
  PROBE_DIR="$(mktemp -d "$ROOT/probe.XXXXXX")"
  # Resolve once in this (non-subshell) parent so LAUNCHER_SOURCE reaches the
  # final report; bounded_omp also resolves internally (each call runs in its
  # own subshell, so LAUNCHER_SOURCE there never propagates back), but both
  # calls land on the same shared launcher cache and the same content-addressed
  # file (fix cycle 5, LOW-3): no separate, unused copy into PROBE_DIR.
  resolve_launcher "$ENGINE_ROOT" "$(launcher_dir_for)" || stop skipped "$REASON" 3
  # Provisional login signal; Step 3 must confirm this after the owner's login.
  if models="$(bounded_omp 9 models cursor --json)"; then rc=0; else rc="$?"; fi
  case "$rc" in
    124 | 137 | 142) stop skipped 'Cursor login check timed out' 3 ;;
  esac
  if [ "$rc" -ne 0 ] || ! printf '%s' "$models" | jq -e '.models | type == "array" and length > 0' >/dev/null 2>&1; then
    stop skipped 'omp --profile glen-harness not logged into Cursor (run: omp --profile glen-harness login cursor)' 3
  fi
  if value="$(bounded_omp 4 config get modelRoles --json)"; then rc=0; else rc="$?"; fi
  case "$rc" in
    124 | 137 | 142) stop skipped 'modelRoles.default config check timed out' 3 ;;
  esac
  [ "$rc" -eq 0 ] || stop skipped 'modelRoles.default config check failed' 3
  MODEL="$(printf '%s' "$value" | jq -er '.value.default | select(type == "string" and length > 0)')" || \
    stop skipped 'modelRoles.default config check failed' 3
  # Item 10 (fix cycle 4): setup persists "<id>:<level>" on the ladder path
  # (omp honours this suffix itself) and a bare id for an explicit --model.
  # The per-dispatch --thinking repeats exactly that persisted level, and is
  # omitted when there is none (fix cycle 6, Claude MED-2 / Codex P2): ladder
  # membership of the id says nothing about what setup saved. Only a trailing
  # :<level> that is one of omp's own levels counts as a suffix.
  THINKING_LEVEL=''
  case "$MODEL" in
    *:off | *:minimal | *:low | *:medium | *:high | *:xhigh | *:max | *:auto)
      THINKING_LEVEL="${MODEL##*:}"
      MODEL="${MODEL%:*}"
      ;;
  esac
  STATUS=ready REASON='' EXIT_CODE=0
}

fingerprint_source() {
  # Untracked files' bytes too (fix cycle 6, Codex P2): status names them and
  # diff HEAD omits them, so an untracked edit mid-capture changed neither.
  # The commit id leads (fix cycle 7, Codex P2-a / Claude LOW-1): a clean
  # checkout or reset leaves status, diff HEAD and the untracked list empty on
  # both sides, so only HEAD tells the two states apart. Output: "<commit> <hash>".
  local head
  head="$(git -C "$1" rev-parse --verify HEAD)" || return 1
  printf '%s ' "$head"
  {
    git -C "$1" status --porcelain=v1 -z || return 1
    git -C "$1" diff HEAD || return 1
    git -C "$1" ls-files -z --others --exclude-standard | while IFS= read -r -d '' path; do
      printf '%s\0' "$path"
      if [ -L "$1/$path" ]; then readlink "$1/$path"; elif [ -f "$1/$path" ]; then shasum -a 256 < "$1/$path"; fi
    done
  } | shasum -a 256 | awk '{print $1}'
}
validate_snapshot_links() {
  # Check every link without following directory links during traversal. abs_path
  # resolves the full chain; unresolved (including dangling/cyclic) links fail closed.
  perl -MCwd=abs_path -MFile::Find -e '
    my $root = shift;
    find({no_chdir => 1, wanted => sub {
      return unless -l $_;
      my $target = abs_path($_);
      die "unsafe snapshot symlink: $_\n" unless defined($target) && -e $target &&
        ($target eq $root || index($target, "$root/") == 0);
      # An alias into quarantine would expose dotenv files at another cwd.
      die "snapshot symlink aliases dotenv quarantine: $_\n" if
        $target eq "$root/_agent-config/dotenv" || index($target, "$root/_agent-config/dotenv/") == 0;
    }}, $root);
  ' "$1" || stop skipped 'snapshot contains unsafe symlink' 3
}
snapshot_command() {
  local SRC="$1" DEST="$2" before after attempt name size path hash dotenv_ready=0 relative
  SRC="$(cd "$SRC" && pwd -P)"
  git -C "$SRC" rev-parse --show-toplevel >/dev/null
  # This command owns only an absent or empty destination, including on recapture.
  mkdir -p "$DEST"
  DEST="$(cd "$DEST" && pwd -P)"
  if under "$DEST" "$SRC" || under "$SRC" "$DEST"; then stop failed 'snapshot paths overlap' 4; fi
  [ -z "$(find "$DEST" -mindepth 1 -maxdepth 1 -print -quit)" ] || stop failed 'snapshot destination is not empty' 4
  validate_snapshot_ancestors "$DEST"
  for attempt in 1 2; do
    before="$(fingerprint_source "$SRC")"
    (
      cd "$SRC"
      git ls-files -z --cached --others --exclude-standard |
        while IFS= read -r -d '' path; do
          if [ -e "$path" ] || [ -L "$path" ]; then printf '%s\0' "$path"; fi
        done | tar --exclude=.git --exclude=.wrangler --null -T - -cf - | tar -xf - -C "$DEST"
    )
    # Test-only race injection exists exclusively in this snapshot subcommand.
    if [ -n "${GLEN_OMP_ENGINE_TEST_MUTATE:-}" ]; then
      (cd "$SRC"; eval "$GLEN_OMP_ENGINE_TEST_MUTATE") >&2
    fi
    after="$(fingerprint_source "$SRC")"
    SOURCE_FINGERPRINT="$after"
    [ "$before" != "$after" ] || break
    find "$DEST" -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +
    [ "$attempt" -ne 2 ] || stop skipped 'source changed during capture' 3
    log 'source changed during capture; recapturing once'
  done
  validate_snapshot_links "$DEST"
  # Measure precisely the copied tree, excluding ignored build output and metadata.
  size="$(du -sk "$DEST" | awk '{print $1}')"
  [ "$size" -le 512000 ] || stop skipped 'snapshot over 500 MB' 3
  # Bun reads .env, .env.local, .env.<mode>, .env.<mode>.local before settings;
  # utils/env.ts also eagerly parses cwd/.env regardless of Bun startup flags.
  # Glen executes omp directly, not `bun --no-env-file ...`; there is no omp
  # dispatch switch suppressing both loaders. Reserve a fresh dotenv subtree and
  # preserve original relative paths there, before root config relocation. Never
  # merge with a copied subtree or overwrite evidence on a name collision.
  while IFS= read -r -d '' path; do
    if [ "$dotenv_ready" -eq 0 ]; then
      [ ! -L "$DEST/_agent-config" ] || stop failed 'agent-config destination is a symlink' 4
      mkdir -p "$DEST/_agent-config"
      [ ! -e "$DEST/_agent-config/dotenv" ] && [ ! -L "$DEST/_agent-config/dotenv" ] || \
        stop failed 'dotenv relocation collision' 4
      mkdir "$DEST/_agent-config/dotenv"
      dotenv_ready=1
    fi
    relative="${path#"$DEST/"}"
    mkdir -p "$DEST/_agent-config/dotenv/$(dirname "$relative")"
    mv "$path" "$DEST/_agent-config/dotenv/$relative"
  done < <(find "$DEST" -path "$DEST/_agent-config/dotenv" -prune -o \
    \( -type f -o -type l \) \( -iname .env -o -iname '.env.*' \) -print0)
  for name in .omp .pi .claude .codex .gemini .cursor .opencode .windsurf .vscode .agent .agents .clinerules .mcp.json mcp.json opencode.json opencode.jsonc; do
    if [ -e "$DEST/$name" ] || [ -L "$DEST/$name" ]; then
      [ ! -L "$DEST/_agent-config" ] || stop failed 'agent-config destination is a symlink' 4
      mkdir -p "$DEST/_agent-config"
      if [ -e "$DEST/_agent-config/${name#.}" ] || [ -L "$DEST/_agent-config/${name#.}" ]; then
        stop failed 'agent-config relocation collision' 4
      fi
      mv "$DEST/$name" "$DEST/_agent-config/${name#.}"
      MOVED="$(printf '%s' "$MOVED" | jq -c --arg name "$name" '. + [$name]')"
    fi
  done
  # Moving a relative link can change its resolved target; re-check before reads.
  validate_snapshot_links "$DEST"
  # Match omp 18.14.0 discovery/at-imports.ts boundaries and path-leading chars.
  # Prefix @ with a literal backslash: the preceding char is then neither a
  # line start nor space/tab. Keep the text (also in code examples) readable.
  # Include symlinked context files and every copilot basename, since a .github
  # directory alias may expose a differently named directory's context file.
  # No WATCHDOG provider exists in discovery/*.ts. Separate advisor loaders read
  # WATCHDOG.md (advisor/watchdog.ts:137-141) and WATCHDOG.yml (advisor/config.ts:193-201).
  # Neutralizing their @ text is speculative defense-in-depth, not a confirmed
  # context-provider @-expansion surface. BSD/macOS find supports -iname too.
  while IFS= read -r -d '' path; do
    [ -f "$path" ] || continue
    perl -e '
      my $file = shift;
      open my $in, "<", $file or die "$file: $!";
      local $/; my $text = <$in>; close $in or die "$file: $!";
      $text =~ s/(^|[ \t])\@(?=[.\/~A-Za-z0-9_-])/$1\\\@/mg;
      open my $out, ">", $file or die "$file: $!";
      print {$out} $text; close $out or die "$file: $!";
    ' "$path"
  done < <(find "$DEST" \( -type f -o -type l \) \
    \( -iname AGENTS.md -o -iname CLAUDE.md -o -iname copilot-instructions.md \
      -o -iname WATCHDOG.md -o -iname WATCHDOG.yml \) -print0)
  # The commit the matching fingerprints recorded, never a later re-read
  # (fix cycle 7): head and source_fingerprint describe the same state.
  HEAD="${SOURCE_FINGERPRINT%% *}"
  SNAPSHOT_SHA256="$(
    cd "$DEST" || exit 1
    find . -type f | LC_ALL=C sort | while IFS= read -r path; do
      hash="$(shasum -a 256 < "$path")" || exit 1
      printf '%s  %s\n' "${hash%% *}" "${path#./}"
    done | shasum -a 256 | awk '{print $1}'
  )"
  STATUS=ok REASON='' EXIT_CODE=0
}

case "$MODE" in
  probe) [ "$#" -eq 0 ] || usage; probe_checks; exit 0 ;;
  snapshot) [ "$#" -eq 2 ] || usage; snapshot_command "$1" "$2"; exit 0 ;;
  review | debug) ;;
  *) usage ;;
esac

REPO="$PWD" BASE='' UNCOMMITTED=0 SCOPE='all changed behavior' BRIEF=''
if [ "$MODE" = review ]; then DEADLINE=15m; else DEADLINE=20m; fi
while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo | --base | --out | --scope | --deadline | --brief)
      [ "$#" -ge 2 ] && [ -n "$2" ] || usage
      case "$1" in
        --repo) REPO="$2" ;;
        --base) BASE="$2" ;;
        --out) OUT="$2" ;;
        --scope) [ "$MODE" = review ] || usage; SCOPE="$2" ;;
        --deadline) DEADLINE="$2" ;;
        --brief) [ "$MODE" = debug ] || usage; BRIEF="$2" ;;
      esac
      shift 2
      ;;
    --uncommitted) [ "$MODE" = review ] || usage; UNCOMMITTED=1; shift ;;
    --keep) KEEP=1; shift ;;
    *) usage ;;
  esac
done
[ -n "$OUT" ] || usage
if [ "$MODE" = review ]; then
  if [ "$UNCOMMITTED" -eq 1 ]; then [ -z "$BASE" ] || usage; else [ -n "$BASE" ] || usage; fi
else
  [ -n "$BRIEF" ] && [ -f "$BRIEF" ] || usage
  case "$BRIEF" in /*) ;; *) BRIEF="$PWD/$BRIEF" ;; esac
fi
if [[ "$DEADLINE" =~ ^([0-9]{1,6})(s|m|h)$ ]]; then
  DEADLINE_SECONDS=$((10#${BASH_REMATCH[1]}))
  case "${BASH_REMATCH[2]}" in m) DEADLINE_SECONDS=$((DEADLINE_SECONDS * 60)) ;; h) DEADLINE_SECONDS=$((DEADLINE_SECONDS * 3600)) ;; esac
else
  usage
fi
[ "$DEADLINE_SECONDS" -gt 0 ] && [ "$DEADLINE_SECONDS" -le 86400 ] || usage
mkdir -p "$(dirname "$OUT")"
OUT="$(cd "$(dirname "$OUT")" && pwd -P)/$(basename "$OUT")"
OUTPUT_READY=1
probe_checks
STATUS=failed REASON='engine command failed' EXIT_CODE=4
REPO="$(git -C "$REPO" rev-parse --show-toplevel)"
# Validate the base early; the merge-base itself is taken from the snapshot's
# own commit below (fix cycle 7), never from whatever HEAD is by then.
[ -z "$BASE" ] || git -C "$REPO" rev-parse --verify --quiet "$BASE^{commit}" >/dev/null || \
  stop failed "base ref $BASE does not name a commit" 4

# Prune only this engine's old run directories, never a caller-supplied tree.
case "$ROOT" in /*/glen-omp-engine) ;; *) stop skipped 'unsafe run root' 3 ;; esac
[ -n "$ROOT" ] || stop skipped 'unsafe run root' 3
find "$ROOT" -mindepth 1 -maxdepth 1 -type d -name 'run-*' -mtime +0 -exec rm -rf -- {} +
RUN_DIR="$ROOT/run-$(date -u +%Y%m%dT%H%M%SZ)-$$"
mkdir -m 0700 "$RUN_DIR"
RUN_GLEN_HOME="$RUN_DIR/glen-home"
POLICY_PATH="$RUN_DIR/policy.yml"
TREE="$RUN_DIR/tree"
policy "$ENGINE_ROOT" "$POLICY_PATH"

# Invoke the same snapshot subcommand used by smoke tests; its JSON stays private.
if env -u GLEN_OMP_ENGINE_TEST_MUTATE "$SCRIPT" snapshot "$REPO" "$TREE" > "$RUN_DIR/snapshot.json"; then SNAP_RC=0; else SNAP_RC="$?"; fi
if [ "$SNAP_RC" -ne 0 ]; then
  SNAP_REASON="$(jq -r '.reason // "snapshot failed"' "$RUN_DIR/snapshot.json")"
  if [ "$SNAP_RC" -eq 3 ]; then stop skipped "$SNAP_REASON" 3; else stop failed "$SNAP_REASON" 4; fi
fi
HEAD="$(jq -r .head "$RUN_DIR/snapshot.json")"
# Bind the merge-base to the commit the snapshot fingerprint recorded; the
# post-diff fingerprint check below catches HEAD moving after the copy.
if [ -n "$BASE" ]; then MERGE_BASE="$(git -C "$REPO" merge-base "$HEAD" "$BASE")"; else MERGE_BASE="$HEAD"; fi
SNAPSHOT_SHA256="$(jq -r .snapshot_sha256 "$RUN_DIR/snapshot.json")"
MOVED="$(jq -c .moved "$RUN_DIR/snapshot.json")"
[ ! -e "$TREE/_review" ] && [ ! -L "$TREE/_review" ] || stop failed 'snapshot contains reserved _review path' 4
mkdir -p "$TREE/_review"
git -C "$REPO" ls-files -z --others --exclude-standard > "$RUN_DIR/untracked.z"
{
  git -C "$REPO" diff --name-only "$MERGE_BASE"
  while IFS= read -r -d '' path; do printf '%s\n' "$path"; done < "$RUN_DIR/untracked.z"
} > "$TREE/_review/changed-files.txt"
if [ "$MODE" = review ] || [ -n "$BASE" ]; then
  git -C "$REPO" diff "$MERGE_BASE" > "$TREE/_review/diff.patch"
  while IFS= read -r -d '' path; do
    if (cd "$REPO"; git diff --no-index -- /dev/null "$path") >> "$TREE/_review/diff.patch"; then
      diff_rc=0
    else
      diff_rc="$?"
    fi
    [ "$diff_rc" -le 1 ] || stop failed 'could not capture untracked diff' 4
  done < "$RUN_DIR/untracked.z"
fi
# The changed-file list and patches come from the live repository after the
# copy: they must still describe the same source state the snapshot fixed
# (fix cycle 6, Codex item-5 P2), or the reviewer reads a tree and a diff
# that disagree.
if [ "$(fingerprint_source "$REPO")" != "$(jq -r '.source_fingerprint // empty' "$RUN_DIR/snapshot.json")" ]; then
  stop skipped 'source changed during capture' 3
fi
[ "$MODE" != debug ] || cp "$BRIEF" "$TREE/_review/brief.md"
cat "$ENGINE_ROOT/.claude/skills/omp/templates/$MODE.md" > "$RUN_DIR/prompt.md"
{
  printf '\n## Run context\n\nBase: %s\nMerge-base SHA: %s\nScope: %s\n' "${BASE:-HEAD (uncommitted)}" "$MERGE_BASE" "$SCOPE"
  printf '\nChanged files (first 200):\n'
  sed -n '1,200p' "$TREE/_review/changed-files.txt"
  if [ "$(wc -l < "$TREE/_review/changed-files.txt")" -gt 200 ]; then printf 'More files: see changed-files.txt.\n'; fi
  printf '\n_agent-config mapping: root names lose their leading dot; other names stay unchanged.\nMoved names:\n'
  printf '%s' "$MOVED" | jq -r '.[] | "\(.) -> _agent-config/\(ltrimstr("."))"'
  printf 'Dotenv files: original relative path -> _agent-config/dotenv/<original relative path>.\n'
} >> "$RUN_DIR/prompt.md"

ENVELOPE="$RUN_DIR/envelope.json"
load_envelope() {
  local spill value
  jq -e 'type == "object"' "$RUN_DIR/response.raw" >/dev/null || return 1
  spill="$(jq -r '.spilled.path // empty' "$RUN_DIR/response.raw")" || return 1
  if [ -n "$spill" ]; then
    cat "$spill" > "$ENVELOPE" || return 1
  else
    cp "$RUN_DIR/response.raw" "$ENVELOPE" || return 1
  fi
  jq -e 'type == "object"' "$ENVELOPE" >/dev/null || return 1
  value="$(jq -r '.data.job.handle // .error.details.job.handle // empty' "$ENVELOPE")" || return 1
  if [ -n "$value" ]; then
    JOB="$value"
    STATUS=running REASON=''
  fi
  value="$(jq -r '.data.session.handle // empty' "$ENVELOPE")" || return 1
  [ -z "$value" ] || SESSION_HANDLE="$value"
  value="$(jq -r '.data.session.path // empty' "$ENVELOPE")" || return 1
  [ -z "$value" ] || SESSION_PATH="$value"
  # Persist before further waits, including interrupted/running envelopes.
  [ -z "$JOB" ] || save_result || return 1
}

FIRST_SEND="$(date +%s)"
JOB_TIMEOUT_SECONDS="$DEADLINE_SECONDS"
RETRIED=0
while :; do
  # Re-check immediately before every new omp cwd use, including capacity retry.
  validate_snapshot_ancestors "$TREE"
  JOB_ACTIVE=1
  # Fix cycle 5, decision 7: omit --thinking entirely for an off-ladder
  # (explicit --model) configuration, rather than guessing a ladder level.
  # Bash 3.2 (macOS's default) treats "${arr[@]}" on a zero-element array as
  # unbound under set -u, so branch on THINKING_LEVEL instead of expanding an
  # possibly-empty THINKING_ARGS array.
  if [ -n "$THINKING_LEVEL" ]; then
    if dispatch_env "$GLEN_BIN" --profile "$PROFILE" send @new - --cwd "$TREE" --thinking "$THINKING_LEVEL" --wait 30s --json \
      < "$RUN_DIR/prompt.md" > "$RUN_DIR/response.raw"; then RC=0; else RC="$?"; fi
  else
    if dispatch_env "$GLEN_BIN" --profile "$PROFILE" send @new - --cwd "$TREE" --wait 30s --json \
      < "$RUN_DIR/prompt.md" > "$RUN_DIR/response.raw"; then RC=0; else RC="$?"; fi
  fi
  load_envelope || stop failed 'invalid glen envelope' 4
  while [ "$RC" -eq 10 ] || [ "$RC" -eq 130 ]; do
    [ -n "$JOB" ] || stop failed 'running glen result has no job handle' 4
    # This deadline can only time out an already identified, recoverable job.
    remaining=$((DEADLINE_SECONDS + 75 - ($(date +%s) - FIRST_SEND)))
    [ "$remaining" -gt 0 ] || stop timeout 'engine deadline exceeded' 5
    wait_seconds="$remaining"
    [ "$wait_seconds" -le 60 ] || wait_seconds=60
    [ "$wait_seconds" -ge 1 ] || wait_seconds=1
    if dispatch_env "$GLEN_BIN" wait "$JOB" --timeout "${wait_seconds}s" --json > "$RUN_DIR/response.raw"; then RC=0; else RC="$?"; fi
    load_envelope || stop failed 'invalid glen envelope' 4
  done
  JOB_ACTIVE=0
  if [ "$RC" -eq 7 ] && jq -e '.error.details.error.code == "timeout"' "$ENVELOPE" >/dev/null; then
    [ -n "$JOB" ] || stop failed 'job timeout result has no job handle' 4
    stop timeout 'glen job deadline exceeded' 5
  fi
  remaining=$((DEADLINE_SECONDS - ($(date +%s) - FIRST_SEND)))
  if [ "$RC" -eq 7 ] && [ "$RETRIED" -eq 0 ] && [ "$remaining" -ge 300 ] && \
    jq -e '[.error.message, .error.details.error.code, .error.details.error.message] |
      map(select(type == "string")) | join(" ") |
      test("429|rate.?limit|capacity|quota|overloaded"; "i")' "$ENVELOPE" >/dev/null; then
    log 'capacity failure; retrying once after 60 seconds in a new session'
    # The failed attempt's job is dead: forget it before the backoff, so an
    # interruption is a failure rather than a recoverable timeout and FILE.json
    # never advertises that job as running (fix cycle 6, omp item-5 finding).
    JOB='' SESSION_HANDLE='' SESSION_PATH=''
    STATUS=failed REASON='capacity failure; retrying once in a new session' EXIT_CODE=4
    rm -f "$ENVELOPE"
    save_result || stop failed 'could not write result JSON' 4
    sleep 60
    JOB_TIMEOUT_SECONDS=$((DEADLINE_SECONDS - ($(date +%s) - FIRST_SEND)))
    RETRIED=1
    continue
  fi
  if [ "$RC" -eq 0 ] || [ "$RC" -eq 11 ]; then
    STATUS=ok REASON='' EXIT_CODE=0
  else
    STATUS=failed REASON="glen dispatch failed (exit $RC)" EXIT_CODE=4
  fi
  break
done
exit "$EXIT_CODE"
