#!/usr/bin/env bash
# Isolated third review/debug engine. Policy/envcheck are internal smoke helpers.
set -euo pipefail
umask 077

MODE="${1:-}"
[ "$#" -eq 0 ] || shift
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
ENGINE_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"
SCRIPT="$SCRIPT_DIR/omp-engine.sh"
PROFILE=glen-harness
START="$(date +%s)"
STATUS=failed REASON='engine command failed' EXIT_CODE=4
MODEL='' PROVIDER='' POLICY_SOURCE='' HEAD='' MERGE_BASE='' SNAPSHOT_SHA256=''
RUN_DIR='' RUN_GLEN_HOME='' POLICY_PATH='' JOB='' SESSION_HANDLE='' SESSION_PATH=''
OUT='' PROBE_DIR='' ENVELOPE='' TREE='' ROOT='' MOVED='[]' REPLIES=0 KEEP=0
OUTPUT_READY=0 JOB_ACTIVE=0 JOB_TIMEOUT_SECONDS=0
USAGE='{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost_total":0}'

log() { printf 'omp-engine: %s\n' "$*" >&2; }
stop() { STATUS="$1"; REASON="$2"; EXIT_CODE="$3"; exit "$EXIT_CODE"; }
usage() {
  log 'usage: omp-engine.sh probe | review [--repo DIR] (--base REF | --uncommitted) --out FILE [--scope TEXT] [--deadline 15m] [--keep]'
  log '       omp-engine.sh debug [--repo DIR] --brief FILE --out FILE [--base REF] [--deadline 20m] [--keep]'
  log '       omp-engine.sh snapshot SRC DEST | policy SRC | envcheck -- CMD...'
  stop failed 'usage error' 2
}

# The single environment boundary for send, wait, and the envcheck smoke helper.
# Mnemosyne credentials pass through untouched; never log environment values.
dispatch_env() {
  env -u CLAUDE_CONFIG_DIR -u PI_PY -u PI_JS -u PI_CODING_AGENT_DIR \
    -u OMP_CODING_AGENT_DIR -u OMP_PROFILE -u PI_PROFILE \
    GLEN_HOME="$RUN_GLEN_HOME" PI_CONFIG_FILES="$POLICY_PATH" \
    GLEN_JOB_TIMEOUT="${JOB_TIMEOUT_SECONDS}s" "$@"
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

# These two internal helpers deliberately expose the requested command/YAML output.
case "$MODE" in
  envcheck)
    if [ "${1:-}" != -- ] || [ "$#" -lt 2 ]; then
      printf '%s\n' '{"v":1,"engine":"omp","mode":"envcheck","status":"failed","reason":"usage error"}'
      exit 2
    fi
    shift
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
    --arg head "$HEAD" --arg merge_base "$MERGE_BASE" --arg snapshot_sha256 "$SNAPSHOT_SHA256" \
    --arg glen_home "$RUN_GLEN_HOME" --arg job "$JOB" --arg handle "$SESSION_HANDLE" \
    --arg path "$SESSION_PATH" --arg out "$OUT" --arg run_dir "$RUN_DIR" \
    --argjson elapsed_ms "$elapsed" --argjson usage "$USAGE" --argjson replies "$REPLIES" \
    --argjson moved "$MOVED" '
    def nullable: if . == "" then null else . end;
    if $mode == "snapshot" then
      {v:1,engine:"omp",mode:$mode,status:$status,reason:($reason|nullable),head:($head|nullable),
       snapshot_sha256:($snapshot_sha256|nullable),moved:$moved}
    else
      {v:1,engine:"omp",mode:$mode,status:$status,reason:($reason|nullable),profile:"glen-harness",
       model:($model|nullable),provider:($provider|nullable),policy_source:($policy_source|nullable),
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
  local meta last_line
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
    MODEL="$(printf '%s' "$meta" | jq -r .model)"
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
  if [ "$shell_status" -ne 0 ] && [ "$JOB_ACTIVE" -eq 1 ] && [ "$STATUS" != timeout ]; then
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
  result_json
  exit "$EXIT_CODE"
}
trap finish EXIT
trap 'stop timeout "dispatch interrupted; job may still be running" 5' INT TERM

under() { case "$1/" in "$2/"*) return 0 ;; *) return 1 ;; esac; }
prepare_root() {
  local parent home
  parent="$(cd "${TMPDIR:-/tmp}" && pwd -P)" || return 1
  home="$(cd "$HOME" && pwd -P)" || return 1
  ROOT="$parent/glen-omp-engine"
  if [ -d "$ROOT" ]; then ROOT="$(cd "$ROOT" && pwd -P)" || return 1; fi
  under "$ROOT" "$home" && return 1
  mkdir -p "$ROOT" || return 1
  ROOT="$(cd "$ROOT" && pwd -P)" || return 1
  ! under "$ROOT" "$home"
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
  local seconds="$1" cwd
  shift
  cwd="$(mktemp -d "$PROBE_DIR/call.XXXXXX")" || return 1
  (
    cd "$cwd" || exit 1
    if command -v timeout >/dev/null 2>&1; then
      env -u CLAUDE_CONFIG_DIR -u PI_PY -u PI_JS -u PI_CONFIG_FILES \
        -u PI_CODING_AGENT_DIR -u OMP_CODING_AGENT_DIR -u OMP_PROFILE -u PI_PROFILE \
        timeout "$seconds" omp --profile "$PROFILE" "$@"
    else
      env -u CLAUDE_CONFIG_DIR -u PI_PY -u PI_JS -u PI_CONFIG_FILES \
        -u PI_CODING_AGENT_DIR -u OMP_CODING_AGENT_DIR -u OMP_PROFILE -u PI_PROFILE \
        perl -e 'alarm shift @ARGV; exec @ARGV' "$seconds" omp --profile "$PROFILE" "$@"
    fi
  )
}
probe_checks() {
  local models value rc
  [ "${GLEN_OMP_ENGINE:-}" != off ] || stop skipped 'disabled by GLEN_OMP_ENGINE=off' 3
  resolve_glen || stop skipped 'no glen binary; run .claude/scripts/omp-profile-setup.sh' 3
  command -v omp >/dev/null 2>&1 || stop skipped 'omp not on PATH' 3
  [ -f "$HOME/${PI_CONFIG_DIR:-.omp}/profiles/$PROFILE/agent/config.yml" ] || \
    stop skipped 'glen-harness profile is not set up; run .claude/scripts/omp-profile-setup.sh' 3
  # shellcheck disable=SC2016 # This required diagnostic names HOME, not its value.
  prepare_root || stop skipped 'run root under $HOME; set TMPDIR outside $HOME' 3
  PROBE_DIR="$(mktemp -d "$ROOT/probe.XXXXXX")"
  # Provisional login signal; Step 3 must confirm this after the owner's login.
  if models="$(bounded_omp 9 models cursor --json)"; then rc=0; else rc="$?"; fi
  case "$rc" in
    124 | 137 | 142) stop skipped 'Cursor login check timed out' 3 ;;
  esac
  if [ "$rc" -ne 0 ] || ! printf '%s' "$models" | jq -e '.models | type == "array" and length > 0' >/dev/null 2>&1; then
    stop skipped 'omp --profile glen-harness not logged into Cursor (run: omp --profile glen-harness login cursor)' 3
  fi
  if value="$(bounded_omp 4 config get modelRoles.default --json)"; then rc=0; else rc="$?"; fi
  case "$rc" in
    124 | 137 | 142) stop skipped 'modelRoles.default config check timed out' 3 ;;
  esac
  [ "$rc" -eq 0 ] || stop skipped 'modelRoles.default config check failed' 3
  MODEL="$(printf '%s' "$value" | jq -er '.value | select(type == "string" and length > 0)')" || \
    stop skipped 'modelRoles.default config check failed' 3
  STATUS=ready REASON='' EXIT_CODE=0
}

fingerprint_source() {
  { git -C "$1" status --porcelain=v1 -z || return 1; git -C "$1" diff HEAD || return 1; } |
    shasum -a 256 | awk '{print $1}'
}
snapshot_command() {
  local SRC="$1" DEST="$2" before after attempt name size path hash
  SRC="$(cd "$SRC" && pwd -P)"
  git -C "$SRC" rev-parse --show-toplevel >/dev/null
  # This command owns only an absent or empty destination, including on recapture.
  mkdir -p "$DEST"
  DEST="$(cd "$DEST" && pwd -P)"
  if under "$DEST" "$SRC" || under "$SRC" "$DEST"; then stop failed 'snapshot paths overlap' 4; fi
  [ -z "$(find "$DEST" -mindepth 1 -maxdepth 1 -print -quit)" ] || stop failed 'snapshot destination is not empty' 4
  size="$(du -sk "$SRC" | awk '{print $1}')"
  [ "$size" -le 512000 ] || stop skipped 'snapshot over 500 MB' 3
  for attempt in 1 2; do
    before="$(fingerprint_source "$SRC")"
    (
      cd "$SRC"
      git ls-files -z --cached --others --exclude-standard |
        while IFS= read -r -d '' path; do
          if [ -e "$path" ] || [ -L "$path" ]; then printf '%s\0' "$path"; fi
        done | tar --exclude=.git --null -T - -cf - | tar -xf - -C "$DEST"
    )
    # Test-only race injection exists exclusively in this snapshot subcommand.
    if [ -n "${GLEN_OMP_ENGINE_TEST_MUTATE:-}" ]; then
      (cd "$SRC"; eval "$GLEN_OMP_ENGINE_TEST_MUTATE") >&2
    fi
    after="$(fingerprint_source "$SRC")"
    [ "$before" != "$after" ] || break
    find "$DEST" -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +
    [ "$attempt" -ne 2 ] || stop skipped 'source changed during capture' 3
    log 'source changed during capture; recapturing once'
  done
  for name in .omp .pi .claude .codex .gemini .cursor .opencode .windsurf .vscode .agents .mcp.json mcp.json opencode.json opencode.jsonc; do
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
  HEAD="$(git -C "$SRC" rev-parse HEAD)"
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
if [ -n "$BASE" ]; then MERGE_BASE="$(git -C "$REPO" merge-base HEAD "$BASE")"; else MERGE_BASE="$(git -C "$REPO" rev-parse HEAD)"; fi

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
if "$SCRIPT" snapshot "$REPO" "$TREE" > "$RUN_DIR/snapshot.json"; then SNAP_RC=0; else SNAP_RC="$?"; fi
if [ "$SNAP_RC" -ne 0 ]; then
  SNAP_REASON="$(jq -r '.reason // "snapshot failed"' "$RUN_DIR/snapshot.json")"
  if [ "$SNAP_RC" -eq 3 ]; then stop skipped "$SNAP_REASON" 3; else stop failed "$SNAP_REASON" 4; fi
fi
HEAD="$(jq -r .head "$RUN_DIR/snapshot.json")"
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
[ "$MODE" != debug ] || cp "$BRIEF" "$TREE/_review/brief.md"
cat "$ENGINE_ROOT/.claude/skills/omp/templates/$MODE.md" > "$RUN_DIR/prompt.md"
{
  printf '\n## Run context\n\nBase: %s\nMerge-base SHA: %s\nScope: %s\n' "${BASE:-HEAD (uncommitted)}" "$MERGE_BASE" "$SCOPE"
  printf '\nChanged files (first 200):\n'
  sed -n '1,200p' "$TREE/_review/changed-files.txt"
  if [ "$(wc -l < "$TREE/_review/changed-files.txt")" -gt 200 ]; then printf 'More files: see changed-files.txt.\n'; fi
  printf '\n_agent-config mapping: root names lose their leading dot; other names stay unchanged.\nMoved names:\n'
  printf '%s' "$MOVED" | jq -r '.[] | "\(.) -> _agent-config/\(ltrimstr("."))"'
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
  JOB_ACTIVE=1
  if dispatch_env "$GLEN_BIN" --profile "$PROFILE" send @new - --cwd "$TREE" --wait 30s --json \
    < "$RUN_DIR/prompt.md" > "$RUN_DIR/response.raw"; then RC=0; else RC="$?"; fi
  load_envelope || stop failed 'invalid glen envelope' 4
  while [ "$RC" -eq 10 ] || [ "$RC" -eq 130 ]; do
    [ -n "$JOB" ] || stop failed 'running glen result has no job handle' 4
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
    stop timeout 'glen job deadline exceeded' 5
  fi
  remaining=$((DEADLINE_SECONDS - ($(date +%s) - FIRST_SEND)))
  if [ "$RC" -eq 7 ] && [ "$RETRIED" -eq 0 ] && [ "$remaining" -ge 300 ] && \
    jq -e '[.error.message, (.error.details | tostring)] | join(" ") | test("429|rate.?limit|capacity|quota|overloaded"; "i")' "$ENVELOPE" >/dev/null; then
    log 'capacity failure; retrying once after 60 seconds in a new session'
    sleep 60
    JOB_TIMEOUT_SECONDS=$((DEADLINE_SECONDS - ($(date +%s) - FIRST_SEND)))
    RETRIED=1
    JOB='' SESSION_HANDLE='' SESSION_PATH=''
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
