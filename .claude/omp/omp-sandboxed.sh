#!/bin/bash
# GLEN_OMP_BIN wrapper. The engine supplies OMP_SANDBOX_RUN_ROOT for each job.
set -euo pipefail
umask 077

die() { printf 'omp-sandboxed: %s\n' "$*" >&2; exit 1; }
[ "$(uname -s)" = Darwin ] || die 'this launcher requires macOS sandbox-exec'
command -v sandbox-exec >/dev/null 2>&1 || die 'sandbox-exec not on PATH; refusing to run omp unsandboxed'

REAL_HOME="$(cd "$HOME" && pwd -P)" || die 'could not resolve HOME directory'
export HOME="$REAL_HOME"
[ -n "${OMP_SANDBOX_RUN_ROOT:-}" ] || die 'OMP_SANDBOX_RUN_ROOT is required'
RUN_ROOT_REAL="$(cd "$OMP_SANDBOX_RUN_ROOT" && pwd -P)" || die 'could not resolve run root'
# Repository callers pass the agent directory; legacy callers use glen-harness.
PROFILE_AGENT_DIR="$(cd "${OMP_PROFILE_AGENT_DIR:-$REAL_HOME/.omp/profiles/glen-harness/agent}" && pwd -P)" || die 'could not resolve profile agent directory'
PROFILE_ROOT="$(cd "$PROFILE_AGENT_DIR/.." && pwd -P)" || die 'could not resolve profile root'

# Resolve both ancestor aliases (/var -> /private/var) and the executable's own
# symlink chain. omp is a self-contained Bun-compiled Mach-O; no runtime/package
# directory is needed. OMP_REAL_BIN also lets smoke helpers use this exact policy.
OMP_REAL_BIN_RESOLVED="$(command -v "${OMP_REAL_BIN:-omp}")" || die 'omp binary not on PATH'
links=0
while :; do
  bin_dir="$(cd "$(dirname "$OMP_REAL_BIN_RESOLVED")" && pwd -P)" || die 'could not resolve omp binary'
  OMP_REAL_BIN_RESOLVED="$bin_dir/${OMP_REAL_BIN_RESOLVED##*/}"
  [ -L "$OMP_REAL_BIN_RESOLVED" ] || break
  links=$((links + 1))
  [ "$links" -le 40 ] || die 'too many omp binary symlinks'
  target="$(readlink "$OMP_REAL_BIN_RESOLVED")" || die 'could not read omp binary symlink'
  case "$target" in
    /*) OMP_REAL_BIN_RESOLVED="$target" ;;
    *) OMP_REAL_BIN_RESOLVED="$(dirname "$OMP_REAL_BIN_RESOLVED")/$target" ;;
  esac
done
[ -f "$OMP_REAL_BIN_RESOLVED" ] && [ -x "$OMP_REAL_BIN_RESOLVED" ] || die 'omp binary must be a regular executable file'

# Quote SBPL strings so spaces, backslashes and quotes in paths stay data.
sbpl_path() {
  local value="$1"
  case "$value" in *$'\n'* | *$'\r'*) die 'newline in sandbox path' ;; esac
  value="${value//\\/\\\\}"
  printf '%s' "${value//\"/\\\"}"
}
# Escape a path for use inside an SBPL (regex #"...") literal. #"..." is a raw
# literal, not an ordinary SBPL string: a literal `\"` inside it ends the
# literal early rather than escaping the quote (fix cycle 5, decision 5,
# Codex P2/LOW-4), so a path containing `"` or `\` is refused outright
# instead of attempting to escape it.
sbpl_regex_literal() {
  local value="$1" out="" ch i len
  case "$value" in
    *$'\n'* | *$'\r'*) die 'newline in sandbox path' ;;
    *'"'* | *$'\\'*) die 'sandbox path contains a literal " or \, which an SBPL regex literal cannot carry safely' ;;
  esac
  len=${#value}
  for ((i = 0; i < len; i++)); do
    ch="${value:i:1}"
    case "$ch" in
      '.' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|')
        out="$out\\$ch"
        ;;
      *)
        out="$out$ch"
        ;;
    esac
  done
  printf '%s' "$out"
}
home_sb="$(sbpl_path "$REAL_HOME")"
run_sb="$(sbpl_path "$RUN_ROOT_REAL")"
bin_sb="$(sbpl_path "$OMP_REAL_BIN_RESOLVED")"
agent_sb="$(sbpl_path "$PROFILE_AGENT_DIR")"
profile_sb="$(sbpl_path "$PROFILE_ROOT")"
agent_regex="$(sbpl_regex_literal "$PROFILE_AGENT_DIR")"
# Preserve read-only treatment for other entries already present in the agent
# directory. The write allowlist below is a named list of entries (the state-
# database regex, sessions/, logs/, cache/, run/), never a broad grant: a new
# filename anywhere else in the agent directory is read-only by default.
readonly_agent_filters() {
  local entry entry_sb
  find "$PROFILE_AGENT_DIR" -mindepth 1 -maxdepth 1 -print0 |
    while IFS= read -r -d '' entry; do
      case "${entry##*/}" in
        agent.db* | models.db* | skill-descriptions.db* | sessions | config.yml | config.yml.lock | mcp.json | extensions | skills) continue ;;
      esac
      entry_sb="$(sbpl_path "$entry")" || exit 1
      printf '  (subpath "%s")\n' "$entry_sb"
    done
}
readonly_filters="$(readonly_agent_filters)" || die 'could not inspect read-only agent entries'
# Fix cycle 5, decision 2 (Claude MED-1): derive host:port from the profile's
# Mnemosyne MCP server's own URL field at launch, so the omp reviewer can
# still reach its memory server. Parses only the URL; never prints, logs or
# otherwise touches headers or tokens.
mnemosyne_network_rule() {
  local mcp_file="$PROFILE_AGENT_DIR/mcp.json" url host port scheme var_name var_default
  [ -f "$mcp_file" ] && command -v jq >/dev/null 2>&1 || return 0
  url="$(jq -r '.mcpServers.mnemosyne.url // empty' "$mcp_file" 2>/dev/null)" || return 0
  [ -n "$url" ] || return 0
  # mcp.json ships the literal "${VAR:-default}" template text; omp expands
  # it at its own runtime, so expand it here the same way, reading only the
  # named env var (never logged) or its literal default.
  if [[ "$url" =~ ^\$\{([A-Za-z_][A-Za-z0-9_]*):-(.*)\}$ ]]; then
    var_name="${BASH_REMATCH[1]}"
    # shellcheck disable=SC2034 # read indirectly, by name, inside eval below.
    var_default="${BASH_REMATCH[2]}"
    eval "url=\"\${$var_name:-\$var_default}\""
  fi
  [[ "$url" =~ ^(https?)://([^/:]+)(:([0-9]+))?(/.*)?$ ]] || return 0
  scheme="${BASH_REMATCH[1]}"
  host="${BASH_REMATCH[2]}"
  port="${BASH_REMATCH[4]}"
  if [ -z "$port" ]; then
    if [ "$scheme" = https ]; then port=443; else port=80; fi
  fi
  case "$host" in
    localhost | 127.0.0.1 | ::1) ;;
    *)
      [ "$port" = 443 ] || die "Mnemosyne MCP host $host is neither loopback nor on 443; refusing rather than granting an unbounded outbound host"
      return 0
      ;;
  esac
  # SBPL's (remote tcp "host:port") host component accepts only "*" or the
  # literal "localhost" (host-verified, fix cycle 5); every loopback form
  # (127.0.0.1, ::1, localhost) normalizes to that one literal.
  printf '(allow network-outbound (remote tcp "localhost:%s"))\n' "$port"
}
mnemosyne_rule="$(mnemosyne_network_rule)" || die 'could not derive the Mnemosyne MCP outbound rule'
profile="$(mktemp "$RUN_ROOT_REAL/omp-sandbox.XXXXXX")"
cat > "$profile" <<EOF
(version 1)
(deny default)
;; Required for dyld bootstrap with narrowed reads, including cryptex shared caches.
;; Without Apple's fragment, dyld can abort before emitting any sandbox deny log.
(import "dyld-support.sb")
;; omp execs itself; tool approval independently denies bash/eval/task.
;; S5 fix-cycle-4 (host-verified via a real send, 2026-10-31): execution is
;; limited to the physical omp binary itself; omp execs itself, and tool
;; approval independently denies bash/eval/task.
(allow process-exec* (literal "$bin_sb"))
(allow process-fork)
(allow file-read*
  ;; Snapshot/run tree; also Bun's startup cwd accessibility probe.
  (subpath "$run_sb")
  ;; Dylib dependencies and system frameworks.
  (subpath "/usr")
  (subpath "/System")
  ;; TLS root certificates for the Cursor API.
  (subpath "/private/etc/ssl")
  ;; Runtime device nodes and timezone database.
  (subpath "/dev")
  (subpath "/private/var/db/timezone")
  ;; S5 fix-cycle-4 (host-verified, 2026-10-31): a real send completes with
  ;; no /Library grant at all (denials there are non-fatal); fix cycle 5
  ;; confirmed /Library/Caches/bun does not exist on this host either, so
  ;; dropped the dead grant for it this replaced.
  ;; DNS config files Bun's getaddrinfo (and dscacheutil) consult.
  (literal "/private/etc/hosts")
  (literal "/private/etc/resolv.conf")
  (literal "/private/etc/services")
  ;; The physical, self-contained omp executable.
  (literal "$bin_sb")
  ;; Agent contents are readable; the write allowlist below (not a broad
  ;; subpath grant) governs what omp may write there.
  (subpath "$agent_sb")
  ;; Profile-scoped runtime siblings observed in the host's S3 write-map.
  (subpath "$profile_sb/logs")
  (subpath "$profile_sb/cache")
  (subpath "$profile_sb/run")
  ;; Shared native cache: populated unsandboxed by setup, then read-only here.
  (subpath "$home_sb/.omp/natives")
  ;; Fix cycle 5, decision 9 (LOW-7): the deny log showed a real dispatch
  ;; reading ~/.omp/run/session-owners for lease state (non-fatal when
  ;; denied, but a read grant keeps the lease check accurate instead of
  ;; silently degrading). Shared owner lock/JSON data, no executable code.
  (subpath "$home_sb/.omp/run")
)
;; S4 fix-cycle-4 allowlist (host-verified via a real send, 2026-10-31): omp's
;; own state databases and their WAL/SHM/journal siblings, plus sessions/ and
;; logs/, are the only agent-directory writes a real dispatch needs. Nothing
;; else in the agent or profile directory is writable; the agent and profile
;; directories themselves cannot be renamed, unlinked or replaced (below).
(allow file-write*
  ;; Job artifacts and harness runtime state.
  (subpath "$run_sb")
  (regex #"^$agent_regex/(agent|models|skill-descriptions)\.db(-wal|-shm|-journal)?\$")
  (subpath "$agent_sb/sessions")
  (subpath "$profile_sb/logs")
  (subpath "$profile_sb/cache")
  (subpath "$profile_sb/run")
  ;; Shared owner leases/daemon state: observed lock/JSON data, no executable
  ;; code. This is a documented property, not an execute-bit rule SBPL enforces.
  (subpath "$home_sb/.omp/run")
  ;; Standard stdio sinks. Real pty writes may be denied; glen uses pipes/files.
  (literal "/dev/null")
  (literal "/dev/tty")
)
;; The agent and profile directories themselves must never be renamed,
;; unlinked or replaced; this is why the allow above is a named-entry
;; allowlist, never a (subpath "$agent_sb") write grant.
(deny file-write*
  (literal "$agent_sb")
  (literal "$profile_sb")
  (literal "$agent_sb/config.yml")
  (literal "$agent_sb/config.yml.lock")
  (literal "$agent_sb/mcp.json")
  (subpath "$agent_sb/extensions")
  (subpath "$agent_sb/skills")
  ;; Fix cycle 5, decision 4 (Claude MED-2): deny renaming or unlinking these
  ;; runtime directories themselves, not just their contents, so the child
  ;; cannot swap one out from under a later unsandboxed omp run on this
  ;; profile. Writes to files inside them stay allowed via the subpath grants
  ;; above.
  (literal "$agent_sb/sessions")
  (literal "$profile_sb/logs")
  (literal "$profile_sb/cache")
  (literal "$profile_sb/run")
$readonly_filters
)
;; Fix cycle 5, decision 4 (Claude MED-2): deny creating a symlink anywhere
;; writable. A planted symlink in an allowed path (for example
;; agent/sessions/x -> ~/.ssh) would fail to write inside this sandbox
;; (Seatbelt checks the resolved target), but a later UNSANDBOXED omp run on
;; this profile (setup, login) would follow it and write through it for real.
(deny file-write-create (vnode-type SYMLINK))
;; Fix cycle 4, item 6 (accepted exfiltration residual, documented in
;; SKILL.md under Risks): outbound is restricted to TCP 443 (any host, not
;; just Cursor's) plus the mDNSResponder DNS socket below. omp still needs
;; name resolution and Cursor's API is plain HTTPS/443, so this is the
;; narrowest grant that keeps a real dispatch working; it does not confine
;; traffic to Cursor specifically.
(allow network-outbound (remote tcp "*:443"))
;; Fix cycle 5, decision 2: the Mnemosyne MCP's own loopback port, derived
;; above from the profile's mcp.json at launch. Empty when mcp.json has no
;; Mnemosyne entry, jq is unavailable, or the port is already 443 (covered
;; by the grant above already).
$mnemosyne_rule
;; Apple com.apple.security.XPCTimeStampingService.sb:79: DNS Unix socket.
(allow network-outbound (literal "/private/var/run/mDNSResponder"))
;; S5 fix-cycle-4 (host-verified via a real send and models cursor --json,
;; 2026-10-31): no mach-lookup allow at all. A real dispatch with this removed
;; still completed; the denied lookups seen in the log (opendirectoryd,
;; diagnosticd, FSEvents, PowerManagement, securityd.xpc, SecurityServer) are
;; all non-fatal telemetry/identity probes omp tolerates failing. Explicitly
;; never allowed: com.apple.coreservices.launchservicesd and the
;; com.apple.pasteboard.* family (SBPL global-name has no wildcard, so there
;; is no single deny clause for the family; neither was requested in any
;; trial, and (deny default) already covers both).
;; Bun JavaScript engine CPU/host feature detection.
(allow sysctl-read)
;; Host-verified (log stream, 2026-10-31): dscacheutil/Bun's getaddrinfo denied
;; file-read-metadata on /var and /etc (ancestors of the literal DNS config
;; paths above) and ipc-posix-shm-read-data on the notification-center shared
;; memory DNS-config-change notifications use; without these, name resolution
;; silently returns nothing (no fatal error, just an empty/failed lookup).
(allow file-read-metadata
  (literal "/var")
  (literal "/etc")
  (literal "/private/var")
  (literal "/private/etc")
)
(allow ipc-posix-shm-read-data (ipc-posix-name "apple.shm.notification_center"))
EOF

# The run directory owns the generated policy's lifetime, including retained jobs.
exec sandbox-exec -f "$profile" "$OMP_REAL_BIN_RESOLVED" "$@"
