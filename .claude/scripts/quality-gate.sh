#!/usr/bin/env bash
# glen's deterministic quality gate for the /implement Implementer loop.
#
# Runs the static half of .github/workflows/ci.yml from the repo root:
#   1. cargo fmt --all -- --check                          (blocking)
#   2. cargo clippy --all-targets --locked -- -D warnings   (blocking)
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

if [ "$status" -eq 0 ]; then
  echo "quality-gate: clean" >&2
fi
exit "$status"
