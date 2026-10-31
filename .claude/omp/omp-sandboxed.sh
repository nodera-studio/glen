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
  ;; The physical, self-contained omp executable.
  (literal "$bin_sb")
  ;; Harness config, mcp.json, skill, sessions and extensions.
  (subpath "$home_sb/.omp/profiles/glen-harness")
  ;; Shared native-binary cache populated by any omp profile.
  (subpath "$home_sb/.omp/natives")
)
(allow file-write*
  ;; Job artifacts and the harness profile's own state.
  (subpath "$run_sb")
  (subpath "$home_sb/.omp/profiles/glen-harness")
  ;; Cross-profile owner leases/daemon state and native-binary cache.
  (subpath "$home_sb/.omp/run")
  (subpath "$home_sb/.omp/natives")
  ;; Standard stdio sinks. Real pty writes may be denied; glen uses pipes/files.
  (literal "/dev/null")
  (literal "/dev/tty")
)
;; Cursor API connections.
(allow network-outbound)
;; XPC services needed by dyld/Bun startup.
(allow mach-lookup)
;; Bun JavaScript engine CPU/host feature detection.
(allow sysctl-read)
EOF

# The run directory owns the generated policy's lifetime, including retained jobs.
exec sandbox-exec -f "$profile" "$OMP_REAL_BIN_RESOLVED" "$@"
