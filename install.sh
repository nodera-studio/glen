#!/bin/sh
# Install a verified glen release without sudo.
# Usage: curl -fsSL https://raw.githubusercontent.com/nodera-studio/glen/main/install.sh | sh -s -- [--version X.Y.Z] [--prefix DIR] [--dry-run]
# Exit codes: 0 success, 1 runtime failure, 2 usage error.
# GLEN_INSTALL_BASE_URL overrides the release base URL for mirrors and tests.
set -eu

usage() {
    cat <<'HELP'
Usage: install.sh [--version X.Y.Z] [--prefix DIR] [--dry-run]
  --version V    Install V (an optional leading v is accepted); default: latest
  --prefix DIR   Install to DIR/bin instead of $HOME/.local/bin (never uses sudo)
  --dry-run      Print target, URLs and install path without downloads or writes
  -h, --help     Show this help
Environment: GLEN_INSTALL_BASE_URL overrides https://github.com/nodera-studio/glen/releases
Uninstall: rm <dir>/glen (default dir: $HOME/.local/bin)
HELP
}

fail() {
    printf 'glen-install: error: %s\n' "$*" >&2
    exit 1
}

usage_error() {
    printf 'glen-install: error: %s\n' "$*" >&2
    usage >&2
    exit 2
}

choose_hash() {
    if command -v sha256sum >/dev/null 2>&1; then
        hash_tool=sha256sum
    elif command -v shasum >/dev/null 2>&1; then
        hash_tool=shasum
    elif command -v openssl >/dev/null 2>&1; then
        hash_tool=openssl
    else
        fail 'a SHA-256 tool is required (sha256sum, shasum or openssl)'
    fi
}

hash_file() {
    case "$hash_tool" in
        sha256sum) sha256sum "$1" ;;
        shasum) shasum -a 256 "$1" ;;
        openssl) openssl dgst -sha256 -r "$1" ;;
    esac
}

download() {
    printf 'glen-install: downloading %s\n' "$1" >&2
    if ! curl -fsSL --retry 3 --connect-timeout 15 --max-time 300 "$@"; then
        fail "could not download $1; is a glen release published for $target? Build from source: cargo install --git https://github.com/nodera-studio/glen --locked"
    fi
}

main() {
    ver='' prefix='' dry_run=false has_prefix=false
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --version|--prefix)
                flag=$1
                [ "$#" -ge 2 ] || usage_error "missing value for $flag"
                case "$2" in ''|--*) usage_error "missing value for $flag" ;; esac
                case "$flag" in
                    --version) ver=$2 ;;
                    --prefix) prefix=$2; has_prefix=true ;;
                esac
                shift 2 ;;
            --version=*) ver=${1#*=}; [ -n "$ver" ] || usage_error 'missing value for --version'; shift ;;
            --prefix=*) prefix=${1#*=}; [ -n "$prefix" ] || usage_error 'missing value for --prefix'; has_prefix=true; shift ;;
            --dry-run) dry_run=true; shift ;;
            -h|--help) usage; exit 0 ;;
            *) usage_error "unknown argument: $1" ;;
        esac
    done
    if [ -n "$ver" ]; then
        ver=${ver#v}
        case "$ver" in *[!0-9A-Za-z.-]*) usage_error "invalid version: $ver" ;; esac
        printf '%s\n' "$ver" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$' || usage_error "invalid version: $ver"
    fi
    platform=$(uname -s) || fail 'could not detect platform'
    machine=$(uname -m) || fail 'could not detect architecture'
    case "$machine" in
        x86_64|amd64) cpu=x86_64 ;;
        arm64|aarch64) cpu=aarch64 ;;
        *) cpu=unsupported ;;
    esac
    if [ "$platform" = Darwin ] && [ "$machine" = x86_64 ] &&
        [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || :)" = 1 ]; then
        cpu=aarch64
    fi
    case "$platform/$cpu" in
        Darwin/aarch64) target=aarch64-apple-darwin ;;
        Darwin/x86_64) target=x86_64-apple-darwin ;;
        Linux/x86_64) target=x86_64-unknown-linux-musl ;;
        Linux/aarch64) target=aarch64-unknown-linux-musl ;;
        *) fail "unsupported platform: $platform $machine; supported: aarch64-apple-darwin, x86_64-apple-darwin, x86_64-unknown-linux-musl, aarch64-unknown-linux-musl; build from source: cargo install --git https://github.com/nodera-studio/glen --locked" ;;
    esac
    base=${GLEN_INSTALL_BASE_URL:-https://github.com/nodera-studio/glen/releases}
    while [ "${base%/}" != "$base" ]; do base=${base%/}; done
    asset=glen-$target.tar.gz
    if [ -n "$ver" ]; then
        archive=$base/download/v$ver/$asset
    else
        archive=$base/latest/download/$asset
    fi
    checksum=$archive.sha256
    if [ "$has_prefix" = true ]; then
        dir=$prefix/bin
    else
        [ -n "${HOME:-}" ] || fail 'HOME is unset or empty; choose a directory with --prefix'
        dir=$HOME/.local/bin
    fi
    if [ "$dry_run" = true ]; then
        printf 'target: %s\narchive: %s\nchecksum: %s\ninstall: %s\n' "$target" "$archive" "$checksum" "$dir/glen"
        exit 0
    fi
    command -v curl >/dev/null 2>&1 || fail 'curl is required'
    choose_hash
    command -v tar >/dev/null 2>&1 || fail 'tar is required'
    tmp=$(mktemp -d) || fail 'could not create temporary directory'
    install_tmp=
    trap 'rm -rf "$tmp"; if [ -n "$install_tmp" ]; then rm -f "$install_tmp"; fi' 0
    trap 'exit 1' INT TERM
    case "$base" in
        https://*)
            download "$archive" -o "$tmp/$asset" --proto '=https' --tlsv1.2
            download "$checksum" -o "$tmp/checksum" --proto '=https' --tlsv1.2 ;;
        *)
            download "$archive" -o "$tmp/$asset"
            download "$checksum" -o "$tmp/checksum" ;;
    esac
    hash_file "$tmp/$asset" > "$tmp/hash" || fail "could not hash $asset"
    actual=$(awk 'NR == 1 {print tolower($1); exit}' "$tmp/hash")
    expected=$(awk 'NR == 1 {print tolower($1); exit}' "$tmp/checksum")
    [ -n "$expected" ] && [ "$actual" = "$expected" ] || fail "checksum mismatch for $asset"
    tar -tzf "$tmp/$asset" > "$tmp/entries" || fail "could not list $asset"
    while IFS= read -r entry; do
        case "$entry" in
            "glen-$target/"|"glen-$target/glen"|"glen-$target/LICENSE"|"glen-$target/THIRD-PARTY-LICENSES") ;;
            *) fail "unexpected archive entry $entry" ;;
        esac
    done < "$tmp/entries"
    # GNU and BSD tar both start verbose entries with the file type. Reject
    # links (including hardlinks) and special files before extraction.
    tar -tvzf "$tmp/$asset" > "$tmp/types" || fail "could not list types in $asset"
    while IFS= read -r entry; do
        case "$entry" in
            -*|d*) ;;
            *) fail "unexpected archive entry type: $entry" ;;
        esac
    done < "$tmp/types"
    tar -xzf "$tmp/$asset" -C "$tmp" || fail "could not extract $asset"
    [ -f "$tmp/glen-$target/glen" ] || fail "archive is missing glen-$target/glen"
    if [ -d "$dir" ] && [ ! -w "$dir" ]; then
        fail "$dir is not writable; choose another directory with --prefix (glen never uses sudo)"
    fi
    mkdir -p "$dir" || fail "cannot create $dir; choose another directory with --prefix (glen never uses sudo)"
    [ -w "$dir" ] || fail "$dir is not writable; choose another directory with --prefix (glen never uses sudo)"
    [ ! -L "$dir/glen" ] && [ ! -d "$dir/glen" ] || fail "destination is a symlink or directory: $dir/glen"
    install_tmp=$(mktemp "$dir/.glen.tmp.XXXXXXXXXX") || fail "cannot create staging file in $dir"
    cp "$tmp/glen-$target/glen" "$install_tmp" || fail "cannot copy glen to $dir"
    chmod 0755 "$install_tmp" || fail "cannot set executable mode in $dir"
    installed_version=$("$install_tmp" --version 2>/dev/null) || fail 'staged glen --version failed; leaving existing binary in place'
    [ ! -L "$dir/glen" ] && [ ! -d "$dir/glen" ] || fail "destination is a symlink or directory: $dir/glen"
    mv -f "$install_tmp" "$dir/glen" || fail "cannot install glen in $dir"
    install_tmp=
    [ -f "$dir/glen" ] && [ ! -L "$dir/glen" ] || fail "installed glen is not a regular file: $dir/glen"
    printf 'installed %s to %s\n' "$installed_version" "$dir/glen"
    case ":${PATH:-}:" in
        *:"$dir":*) ;;
        *) printf 'glen-install: add %s to PATH\n' "$dir" >&2 ;;
    esac
}

main "$@"
