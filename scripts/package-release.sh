#!/bin/sh
# Package a glen binary and LICENSE as release assets.
# Usage: scripts/package-release.sh <target> <binary> <out-dir>
# Exit codes: 0 success, 1 runtime failure, 2 usage error.
set -eu

fail() {
    printf 'glen-package: error: %s\n' "$*" >&2
    exit 1
}

usage_error() {
    printf 'glen-package: error: %s\nUsage: scripts/package-release.sh <target> <binary> <out-dir>\n' "$*" >&2
    exit 2
}

hash_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1"
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1"
    elif command -v openssl >/dev/null 2>&1; then
        openssl dgst -sha256 -r "$1"
    else
        fail 'a SHA-256 tool is required (sha256sum, shasum or openssl)'
    fi
}

main() {
    [ "$#" -eq 3 ] || usage_error 'expected exactly three arguments'
    target=$1 binary=$2 out=$3
    case "$target" in *[!a-z0-9_-]*) usage_error "invalid target: $target" ;; esac
    printf '%s\n' "$target" | grep -Eq '^[a-z0-9_]+(-[a-z0-9_]+)+$' || usage_error "invalid target: $target"
    [ -f "$binary" ] && [ -x "$binary" ] || fail "binary is not an executable file: $binary"
    root=$(cd "$(dirname "$0")/.." && pwd) || fail 'cannot find repository root'
    stage=$(mktemp -d) || fail 'cannot create staging directory'
    trap 'rm -rf "$stage"' 0
    trap 'exit 1' INT TERM
    name=glen-$target
    mkdir -p "$stage/$name" "$out" || fail 'cannot create output directories'
    cp "$binary" "$stage/$name/glen" || fail 'cannot stage binary'
    chmod 0755 "$stage/$name/glen" || fail 'cannot set executable mode'
    cp "$root/LICENSE" "$stage/$name/LICENSE" || fail 'cannot stage LICENSE'
    COPYFILE_DISABLE=1 tar -C "$stage" -czf "$out/$name.tar.gz" "$name" || fail 'cannot create archive'
    hash_file "$out/$name.tar.gz" > "$stage/hash" || fail 'cannot hash archive'
    hash=$(awk 'NR == 1 {print tolower($1); exit}' "$stage/hash")
    printf '%s  %s.tar.gz\n' "$hash" "$name" > "$out/$name.tar.gz.sha256"
    printf '%s\n%s\n' "$out/$name.tar.gz" "$out/$name.tar.gz.sha256"
}

main "$@"
