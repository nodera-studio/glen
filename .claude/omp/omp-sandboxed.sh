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
home_sb="$(sbpl_path "$REAL_HOME")"
run_sb="$(sbpl_path "$RUN_ROOT_REAL")"
bin_sb="$(sbpl_path "$OMP_REAL_BIN_RESOLVED")"
agent_sb="$(sbpl_path "$PROFILE_AGENT_DIR")"
profile_sb="$(sbpl_path "$PROFILE_ROOT")"
# Preserve read-only treatment for other entries already present in the agent
# directory. SQLite database families and sessions need directory operations;
# new transient filenames are intentionally covered by the broad write grant.
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
profile="$(mktemp "$RUN_ROOT_REAL/omp-sandbox.XXXXXX")"
cat > "$profile" <<EOF
(version 1)
(deny default)
;; Required for dyld bootstrap with narrowed reads, including cryptex shared caches.
;; Without Apple's fragment, dyld can abort before emitting any sandbox deny log.
(import "dyld-support.sb")
;; omp execs itself; tool approval independently denies bash/eval/task.
(allow process-exec*)
(allow process-fork)
(allow file-read*
  ;; Snapshot/run tree; also Bun's startup cwd accessibility probe.
  (subpath "$run_sb")
  ;; Dylib dependencies and system frameworks.
  (subpath "/usr")
  (subpath "/System")
  (subpath "/Library")
  ;; TLS root certificates for the Cursor API.
  (subpath "/private/etc/ssl")
  ;; Runtime device nodes and timezone database.
  (subpath "/dev")
  (subpath "/private/var/db/timezone")
  ;; DNS config files Bun's getaddrinfo (and dscacheutil) consult.
  (literal "/private/etc/hosts")
  (literal "/private/etc/resolv.conf")
  (literal "/private/etc/services")
  ;; The physical, self-contained omp executable.
  (literal "$bin_sb")
  ;; Agent contents are readable; the later write-deny stanza protects config,
  ;; extensions, skills, and other non-runtime entries present at launch.
  (subpath "$agent_sb")
  ;; Profile-scoped runtime siblings observed in the host's S3 write-map.
  (subpath "$profile_sb/logs")
  (subpath "$profile_sb/cache")
  (subpath "$profile_sb/run")
  ;; Shared native cache: populated unsandboxed by setup, then read-only here.
  (subpath "$home_sb/.omp/natives")
)
(allow file-write*
  ;; Job artifacts and harness runtime state.
  (subpath "$run_sb")
  ;; Host SQLite CREATE/INSERT/SELECT probes require a genuine directory grant
  ;; for journals/locks/renames; literal DB grants (even + directory literal) fail.
  (subpath "$agent_sb")
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
;; Host-verified S3 shape: these later denies override the agent-directory allow.
;; Keep fixed configuration protected even when a file/directory is absent.
(deny file-write*
  (literal "$agent_sb/config.yml")
  (literal "$agent_sb/config.yml.lock")
  (literal "$agent_sb/mcp.json")
  (subpath "$agent_sb/extensions")
  (subpath "$agent_sb/skills")
$readonly_filters
)
;; Isolated S4 host-validation candidate (2026-10-31): Apple parsecd uses bare
;; (remote ip), never the nonmatching (remote ip "*:*") form. The host's earlier
;; deny-default trials failed DNS; sandbox_apply EPERM blocks Codex verification.
;; DNS/chat completion is pending the owner's isolated host check, not proven by
;; cached models output. Other Unix socket paths receive no outbound grant.
(allow network-outbound (remote ip))
;; Apple com.apple.security.XPCTimeStampingService.sb:79: DNS Unix socket.
(allow network-outbound (literal "/private/var/run/mDNSResponder"))
;; Keep XPC lookup broad while the coupled S4 DNS investigation is unresolved.
(allow mach-lookup)
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
