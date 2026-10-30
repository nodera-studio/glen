#!/usr/bin/env bash
# glen's deterministic quality gate for the /implement Implementer loop.
#
# Runs the static half of .github/workflows/ci.yml from the repo root:
#   1. cargo fmt --all -- --check                          (blocking)
#   2. cargo clippy --all-targets --locked -- -D warnings   (blocking)
#   3. the same clippy with --target x86_64-unknown-linux-gnu (blocking), so cfg(linux)
#      code is linted on a macOS dev box: CI's ubuntu runner fails on it otherwise. Check
#      only, no linker needed. Skipped with a warning when that rustup target is missing
#      (`rustup target add x86_64-unknown-linux-gnu`); skipped silently on Linux hosts.
# Tests are run separately by the Implementer (`cargo test --locked`), so a slice can
# report "tests red" and "gate red" as distinct states.
#
# Exit 0 = no blocking findings. Exit 1 = at least one step failed; its output is
# written to target/reports/quality-gate-<step>.txt (git-ignored) and summarised on stderr.
# There is no warn tier: clippy runs with -D warnings, exactly as CI does.
set -uo pipefail

root="$(git rev-parse --show-toplevel 2>/dev/null)" || { echo "quality-gate: not in a git repo" >&2; exit 1; }
cd "$root" || exit 1
mkdir -p target/reports

status=0
run_step() {
  local name="$1"; shift
  if "$@" >"target/reports/quality-gate-$name.txt" 2>&1; then
    echo "quality-gate: $name ok" >&2
  else
    echo "quality-gate: $name FAILED (see target/reports/quality-gate-$name.txt)" >&2
    tail -n 40 "target/reports/quality-gate-$name.txt" >&2
    status=1
  fi
}

run_step fmt cargo fmt --all -- --check
run_step clippy cargo clippy --all-targets --locked -- -D warnings

linux_target=x86_64-unknown-linux-gnu
if [ "$(uname -s)" != "Linux" ]; then
  if rustup target list --installed 2>/dev/null | grep -qx "$linux_target"; then
    run_step clippy-linux cargo clippy --all-targets --locked --target "$linux_target" -- -D warnings
  else
    echo "quality-gate: WARNING clippy-linux skipped: rustup target $linux_target not installed; cfg(linux) code is unlinted (rustup target add $linux_target)" >&2
  fi
fi

if [ "$status" -eq 0 ]; then
  echo "quality-gate: clean" >&2
fi
exit "$status"
