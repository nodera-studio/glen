#!/bin/sh
# Repeat the test suite under CPU load, stopping at the first failure.
# Usage: scripts/flake-check.sh [RUNS] [-- cargo-test-args]
# Exit codes: 0 success, 1 runtime failure, 2 usage error.
set -eu

fail() {
    printf 'glen-flake: error: %s\n' "$*" >&2
    exit 1
}

usage_error() {
    printf 'glen-flake: error: %s\nUsage: scripts/flake-check.sh [RUNS] [-- cargo-test-args]\n' "$*" >&2
    exit 2
}

cleanup() {
    for pid in $burners; do
        kill "$pid" 2>/dev/null || :
    done
    for pid in $burners; do
        wait "$pid" 2>/dev/null || :
    done
}

main() {
    runs=20
    if [ "$#" -gt 0 ] && [ "$1" != -- ]; then
        runs=$1
        shift
    fi
    case "$runs" in ''|*[!0-9]*) usage_error 'RUNS must be a positive integer' ;; esac
    [ "$runs" -gt 0 ] 2>/dev/null || usage_error 'RUNS must be a positive integer'
    if [ "$#" -gt 0 ]; then
        [ "$1" = -- ] || usage_error 'cargo test arguments must follow --'
        shift
    fi

    command -v node >/dev/null 2>&1 || fail 'Node >=22.6 is required to run the extension suites'
    node_version=$(node --version) || fail 'cannot determine the Node version (>=22.6 required)'
    printf '%s\n' "$node_version" | awk -F '[v.]' '/^v[0-9]+\.[0-9]+\.[0-9]+$/ && ($2 > 22 || ($2 == 22 && $3 >= 6)) {ok=1} END {exit !ok}' \
        || fail 'Node >=22.6 is required to run the extension suites'

    n=$(getconf _NPROCESSORS_ONLN) || fail 'cannot determine the CPU count'
    case "$n" in ''|*[!0-9]*) fail "invalid CPU count: $n" ;; esac
    [ "$n" -gt 0 ] || fail "invalid CPU count: $n"
    threads=$((2*n))
    out=${FLAKE_OUT:-target/flake-check}
    mkdir -p "$out" || fail "cannot create output directory: $out"
    head=$(git rev-parse HEAD) || fail 'cannot determine Git HEAD'
    os=$(uname -sm) || fail 'cannot determine the OS'
    {
        printf 'HEAD: %s\n' "$head"
        printf 'OS: %s\n' "$os"
        printf 'n: %s\nthreads: %s\n' "$n" "$threads"
    } > "$out/summary.txt"

    cargo test --locked --no-run "$@" || fail 'cargo test compilation failed'

    burners=
    trap cleanup 0
    trap 'exit 130' INT
    trap 'exit 143' TERM
    i=0
    while [ "$i" -lt "$n" ]; do
        sh -c 'while :; do :; done' &
        burners="$burners $!"
        i=$((i+1))
    done

    run=1
    while [ "$run" -le "$runs" ]; do
        log=$out/run-$run.log
        if cargo test --locked "$@" -- --test-threads="$threads" > "$log" 2>&1; then
            result="run $run: ok"
        else
            names=$(grep '^test .* FAILED$' "$log" | awk '{printf "%s%s", sep, $2; sep=" "}' || :)
            result="run $run: FAILED ${names:-see $log}"
            printf '%s\n' "$result"
            printf '%s\n' "$result" >> "$out/summary.txt"
            exit 1
        fi
        printf '%s\n' "$result"
        printf '%s\n' "$result" >> "$out/summary.txt"
        run=$((run+1))
    done
}

main "$@"
