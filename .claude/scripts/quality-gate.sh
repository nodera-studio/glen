#!/usr/bin/env bash
# glen's deterministic quality gate for the /implement Implementer loop.
#
# Runs the static half of .github/workflows/ci.yml from the repo root:
#   1. cargo fmt --all -- --check                          (blocking)
#   2. cargo clippy --all-targets --locked -- -D warnings   (blocking)
#   3. site check, build, timeline tests, budgets when site/palette changed
#      (Playwright and LHCI remain CI-only.)
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

base=$(git merge-base HEAD origin/main 2>/dev/null || git merge-base HEAD main 2>/dev/null || echo HEAD)
site_changed=$(git diff --name-only "$base" -- site src/render/palette.json; git ls-files --others --exclude-standard -- site)
if [ -n "$site_changed" ]; then
  if [ ! -d site/node_modules ]; then
    echo "quality-gate: site changed but site/node_modules is missing; run 'npm ci' in site/" >&2
    status=1
  else
    run_step site-check npm --prefix site run check
    run_step site-build npm --prefix site run build
    run_step site-unit npm --prefix site run test:unit
    run_step site-budgets npm --prefix site run budgets
  fi
fi

if [ "$status" -eq 0 ]; then
  echo "quality-gate: clean" >&2
fi
exit "$status"
