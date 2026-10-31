#!/bin/bash
# Linux GLEN_OMP_BIN wrapper. The engine supplies OMP_SANDBOX_RUN_ROOT per job.
set -euo pipefail
umask 077

die() { printf 'omp-sandboxed-linux: %s\n' "$*" >&2; exit 1; }
[ "$(uname -s)" = Linux ] || die 'this launcher requires Linux bwrap'
command -v bwrap >/dev/null 2>&1 || die 'bwrap not on PATH; refusing to run omp unsandboxed'

REAL_HOME="$(cd "$HOME" && pwd -P)" || die 'could not resolve HOME directory'
export HOME="$REAL_HOME"
[ -n "${OMP_SANDBOX_RUN_ROOT:-}" ] || die 'OMP_SANDBOX_RUN_ROOT is required'
RUN_ROOT_REAL="$(cd "$OMP_SANDBOX_RUN_ROOT" && pwd -P)" || die 'could not resolve run root'
RUN_CWD="$(pwd -P)" || die 'could not resolve working directory'

# Resolve ancestor aliases and the executable's own symlink chain, as on macOS.
# omp is self-contained; only its executable and system linking dependencies bind.
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

# bwrap starts with an empty root, not a bind of the host root or HOME. A private
# PID namespace keeps /proc from exposing host processes' root/fd references.
# Network stays in the host namespace by default for Cursor's API and DNS.
# Flags: https://github.com/containers/bubblewrap/blob/main/bwrap.xml
binds=(--unshare-user --unshare-pid --die-with-parent --new-session)
# Dynamic linker, libraries, system binaries, and framework/data equivalents.
# Bind each existing path so both merged-/usr and traditional layouts work.
for path in /usr /bin /sbin /lib /lib32 /lib64; do
  if [ -d "$path" ]; then binds+=(--ro-bind "$path" "$path"); fi
done
# TLS roots, DNS/NSS, dynamic linker cache/config, and timezone data. Do not
# expose all of /etc: unrelated host configuration is outside the allowlist.
for path in /etc/ssl /etc/pki /etc/resolv.conf /etc/hosts /etc/nsswitch.conf \
  /etc/gai.conf /etc/ld.so.cache /etc/ld.so.conf /etc/ld.so.conf.d /etc/localtime /etc/timezone; do
  if [ -e "$path" ]; then binds+=(--ro-bind "$path" "$path"); fi
done
binds+=(--ro-bind "$OMP_REAL_BIN_RESOLVED" "$OMP_REAL_BIN_RESOLVED")

# Writable host mounts: job artifacts/snapshot, harness config/MCP/skills/sessions,
# shared native-binary cache, and cross-profile owner leases/daemon runtime state.
# Create only these state directories; refuse aliases that could import another
# profile or arbitrary host data through a bind's source resolution.
for path in "$REAL_HOME/.omp/profiles/glen-harness" "$REAL_HOME/.omp/natives" "$REAL_HOME/.omp/run"; do
  ancestor="$path"
  while [ "$ancestor" != "$REAL_HOME" ]; do
    [ ! -L "$ancestor" ] || die "symlink in sandbox state path: $ancestor"
    ancestor="$(dirname "$ancestor")"
  done
  mkdir -p "$path"
  [ "$(cd "$path" && pwd -P)" = "$path" ] || die "noncanonical sandbox state path: $path"
  binds+=(--bind "$path" "$path")
done
binds+=(--bind "$RUN_ROOT_REAL" "$RUN_ROOT_REAL")
# Fresh device nodes include stdio sinks/randomness; private proc supports Bun's
# host/CPU probes. Other directories in the synthetic root remain read-only.
binds+=(--dev /dev --proc /proc --remount-ro / --chdir "$RUN_CWD")

# Replace the wrapper; setup failure stays fatal, with no unsandboxed fallback.
exec bwrap "${binds[@]}" -- "$OMP_REAL_BIN_RESOLVED" "$@"
