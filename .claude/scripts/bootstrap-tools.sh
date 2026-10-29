#!/usr/bin/env bash
# bootstrap-tools.sh — install the Claude Code productivity CLI layer for this project.
#
# Idempotent + OS-detecting. Run it once per machine (Linux box AND Mac):
#   .claude/scripts/bootstrap-tools.sh            # install what's missing
#   .claude/scripts/bootstrap-tools.sh --dry-run  # show the plan, install nothing
#   .claude/scripts/bootstrap-tools.sh --help
#
# Posture: best-effort. Every tool installs independently; a failure is recorded
# and the run continues, ending with a present/installed/failed summary. Already
# present (skipped): rg, fd/fdfind, jq, fzf, gh, semgrep, bunx, npx.
#
# macOS  -> Homebrew (brew).
# Linux  -> apt for the apt-fresh tools, prebuilt GitHub-release binaries for the
#           rest (this repo's Linux box has no cargo/rustup/brew). yq is fetched
#           as the mikefarah v4 binary, NOT the apt python "yq" (a jq wrapper).
#
# ast-grep installs both `ast-grep` and `sg`. `sg` collides with util-linux's
# group-exec `sg` (/usr/bin/sg) — this script NEVER aliases `ast-grep` to `sg`.
# Invoke it as `ast-grep`. To shorten it, add your own alias `astg` (printed at end).
set -uo pipefail

DRY_RUN=0
case "${1:-}" in
  --dry-run | -n) DRY_RUN=1 ;;
  --help | -h)
    sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
    ;;
  "") ;;
  *)
    echo "Unknown arg: $1 (use --help)" >&2
    exit 2
    ;;
esac

OS="$(uname -s)"
ARCH="$(uname -m)"
PRESENT=() INSTALLED=() FAILED=() PLANNED=()

have() { command -v "$1" >/dev/null 2>&1; }
say() { printf '%s\n' "$*"; }
run() {
  if [ "$DRY_RUN" -eq 1 ]; then
    say "  [dry-run] $*"
    return 0
  fi
  "$@"
}

# Per-machine binary dir for GitHub-release installs (Linux). Prefer a writable
# system dir, else ~/.local/bin (ensure it's on PATH in your shell rc).
BIN_DIR="${HOME}/.local/bin"
if [ -w /usr/local/bin ]; then BIN_DIR="/usr/local/bin"; fi
# --dry-run must not touch the filesystem.
[ "$DRY_RUN" -eq 1 ] || mkdir -p "$BIN_DIR" 2>/dev/null || true

# Single scratch root, auto-removed on exit (no /tmp debris on failed installs).
TMPROOT="$(mktemp -d)"
trap 'rm -rf "$TMPROOT"' EXIT

SUDO=""
if [ "$(id -u)" -ne 0 ] && have sudo; then SUDO="sudo"; fi

# Arch tokens used to build release-asset patterns.
case "$ARCH" in
  x86_64 | amd64)
    RUST_ARCH="x86_64-unknown-linux-gnu"
    GO_ARCH="x86_64"
    AMD="amd64"
    PROCS_ARCH="x86_64-linux"
    GITLEAKS_ARCH="linux_x64"
    ;;
  aarch64 | arm64)
    RUST_ARCH="aarch64-unknown-linux-gnu"
    GO_ARCH="arm64"
    AMD="arm64"
    PROCS_ARCH="aarch64-linux"
    GITLEAKS_ARCH="linux_arm64"
    ;;
  *)
    RUST_ARCH="x86_64-unknown-linux-gnu"
    GO_ARCH="x86_64"
    AMD="amd64"
    PROCS_ARCH="x86_64-linux"
    GITLEAKS_ARCH="linux_x64"
    say "WARN: unrecognized arch '$ARCH' — defaulting to x86_64 patterns."
    ;;
esac

# Download a release asset (latest) matching a glob from a GitHub repo into $1.
# Uses gh if available (handles auth + latest), else the public release API.
# Integrity: assets arrive over authenticated HTTPS from the GitHub releases API;
# per-asset checksum schemes vary too widely across these ~18 repos to verify
# robustly, so checksum verification is intentionally omitted for this user-run
# convenience installer. (Re-evaluate per-tool if a predictable .sha256 sibling exists.)
gh_fetch() { # gh_fetch <outfile> <repo> <asset-glob>
  local out="$1" repo="$2" glob="$3" tmpd
  tmpd="$(mktemp -d -p "$TMPROOT")"
  if have gh; then
    gh release download --repo "$repo" --pattern "$glob" --dir "$tmpd" --clobber >/dev/null 2>&1 || return 1
  else
    # No gh: match asset names with a real glob (not a leaky regex) and skip
    # checksum/signature siblings that would otherwise match a broad pattern.
    local url="" u
    while IFS= read -r u; do
      case "${u##*/}" in
        *.sha256 | *.sha512 | *.asc | *.sig | *.b3) continue ;;
      esac
      # shellcheck disable=SC2254  # $glob is intentionally a glob pattern here
      case "${u##*/}" in
        $glob)
          url="$u"
          break
          ;;
      esac
    done < <(curl -fsSL "https://api.github.com/repos/${repo}/releases/latest" |
      grep -oE '"browser_download_url": *"[^"]+"' | cut -d'"' -f4)
    [ -n "$url" ] || return 1
    curl -fsSL "$url" -o "$tmpd/asset" || return 1
  fi
  local f
  f="$(find "$tmpd" -type f | head -1)"
  [ -n "$f" ] && mv "$f" "$out" && return 0
  return 1
}

# Extract any archive into a fresh dir and echo that dir. The type is taken from
# the <hint> (the asset glob/name) because <archive> is a temp file with a generic
# .dl suffix — switching on the temp name would never match and never extract.
unpack() { # unpack <archive> <hint-with-real-extension>
  local arc="$1" hint="${2:-$1}" d
  d="$(mktemp -d -p "$TMPROOT")"
  case "$hint" in
    *.tar.gz | *.tgz) tar -xzf "$arc" -C "$d" 2>/dev/null ;;
    *.tar.xz) tar -xJf "$arc" -C "$d" 2>/dev/null ;;
    *.tar.bz2) tar -xjf "$arc" -C "$d" 2>/dev/null ;;
    *.zip) unzip -qo "$arc" -d "$d" 2>/dev/null ;;
    *) cp "$arc" "$d/" ;;
  esac
  printf '%s' "$d"
}

# Install one named binary found anywhere inside an unpacked dir.
place_bin() { # place_bin <dir> <binname>
  local d="$1" name="$2" src
  src="$(find "$d" -type f -name "$name" 2>/dev/null | head -1)"
  [ -n "$src" ] || return 1
  chmod +x "$src" 2>/dev/null || true
  run install -m 0755 "$src" "$BIN_DIR/$name" || run cp "$src" "$BIN_DIR/$name"
}

# Generic GitHub-release tool install: fetch archive, unpack, place binary.
gh_install() { # gh_install <cmd> <repo> <asset-glob> [binname]
  local cmd="$1" repo="$2" glob="$3" bin="${4:-$1}" arc dir
  arc="$(mktemp -p "$TMPROOT" --suffix=.dl)"
  if ! gh_fetch "$arc" "$repo" "$glob"; then
    FAILED+=("$cmd (download)")
    return 1
  fi
  # Raw single-binary asset (no archive extension in the glob).
  case "$glob" in
    *.tar.* | *.tgz | *.zip) dir="$(unpack "$arc" "$glob")" ;;
    *)
      dir="$(mktemp -d -p "$TMPROOT")"
      cp "$arc" "$dir/$bin"
      ;;
  esac
  if place_bin "$dir" "$bin"; then
    INSTALLED+=("$cmd")
  else
    FAILED+=("$cmd (no '$bin' in asset)")
  fi
}

apt_install() { # apt_install <cmd> <pkg>
  local cmd="$1" pkg="$2"
  if run $SUDO apt-get install -y "$pkg" >/dev/null 2>&1; then
    INSTALLED+=("$cmd")
  else
    FAILED+=("$cmd (apt $pkg)")
  fi
}

brew_install() { # brew_install <cmd> <formula>
  local cmd="$1" formula="$2"
  if run brew install "$formula" >/dev/null 2>&1; then
    INSTALLED+=("$cmd")
  else
    FAILED+=("$cmd (brew $formula)")
  fi
}

# ---- the tool set: cmd -> install plan -----------------------------------
# Focused (high-leverage) + long-tail, per the approved plan.
install_one() { # install_one <cmd>
  local cmd="$1"
  if have "$cmd"; then
    PRESENT+=("$cmd")
    return 0
  fi
  # fd ships as fdfind on Debian — treat either as present.
  if [ "$cmd" = "fd" ] && have fdfind; then
    PRESENT+=("fd (fdfind)")
    return 0
  fi
  PLANNED+=("$cmd")
  # --dry-run computes the plan only — no network, no filesystem writes.
  [ "$DRY_RUN" -eq 1 ] && return 0

  if [ "$OS" = "Darwin" ]; then
    case "$cmd" in
      delta) brew_install delta git-delta ;;
      difft) brew_install difft difftastic ;;
      ast-grep) brew_install ast-grep ast-grep ;;
      mlr) brew_install mlr miller ;;
      ctags) brew_install ctags universal-ctags ;;
      *) brew_install "$cmd" "$cmd" ;;
    esac
    return 0
  fi

  # Linux
  case "$cmd" in
    # apt-fresh on this repo's box
    shellcheck) apt_install shellcheck shellcheck ;;
    delta) apt_install delta git-delta ;;
    ctags) apt_install ctags universal-ctags ;;
    mlr) apt_install mlr miller ;;
    hyperfine) apt_install hyperfine hyperfine ;;
    # mikefarah v4 (NOT apt python yq)
    yq)
      if gh_fetch "$BIN_DIR/yq" mikefarah/yq "yq_linux_${AMD}" && run chmod +x "$BIN_DIR/yq"; then
        INSTALLED+=("yq")
      else FAILED+=("yq"); fi
      ;;
    # prebuilt GitHub-release binaries
    # `*` because assets gained a version segment in 0.71 (difft-0.71.0-x86_64-…); it also matches the older unversioned name
    difft) gh_install difft Wilfred/difftastic "difft-*${RUST_ARCH}.tar.gz" difft ;;
    sd) gh_install sd chmln/sd "sd-*-${RUST_ARCH}.tar.gz" sd ;;
    scc) gh_install scc boyter/scc "scc_Linux_${GO_ARCH}.tar.gz" scc ;; # asset has no version segment
    gron) gh_install gron tomnomnom/gron "gron-linux-${AMD}-*.tgz" gron ;;
    jless) gh_install jless PaulJuliusMartinez/jless "jless-*-${RUST_ARCH}.zip" jless ;;
    git-absorb) gh_install git-absorb tummychow/git-absorb "git-absorb-*-${RUST_ARCH/-gnu/-musl}.tar.gz" git-absorb ;; # upstream ships musl, not gnu
    watchexec) gh_install watchexec watchexec/watchexec "watchexec-*-${RUST_ARCH}.tar.xz" watchexec ;;
    dust) gh_install dust bootandy/dust "dust-*-${RUST_ARCH}.tar.gz" dust ;;
    procs) gh_install procs dalance/procs "procs-*-${PROCS_ARCH}.zip" procs ;;
    hexyl) gh_install hexyl sharkdp/hexyl "hexyl-*-${RUST_ARCH}.tar.gz" hexyl ;;
    lazygit) gh_install lazygit jesseduffield/lazygit "lazygit_*_linux_${GO_ARCH}.tar.gz" lazygit ;; # asset uses lowercase 'linux'
    gitleaks) gh_install gitleaks gitleaks/gitleaks "gitleaks_*_${GITLEAKS_ARCH}.tar.gz" gitleaks ;;
    ast-grep) gh_install ast-grep ast-grep/ast-grep "app-${RUST_ARCH}.zip" ast-grep ;;
    *) FAILED+=("$cmd (no install rule)") ;;
  esac
}

TOOLS=(
  shellcheck gitleaks delta difft ast-grep sd yq # focused / high-leverage
  scc gron jless git-absorb hyperfine             # long-tail (scc covers LOC; tokei
  watchexec dust procs hexyl lazygit mlr ctags    # dropped — no prebuilt binaries upstream)
)

say "bootstrap-tools.sh — OS=$OS ARCH=$ARCH BIN_DIR=$BIN_DIR$([ "$DRY_RUN" -eq 1 ] && echo ' (DRY RUN)')"
say ""

if [ "$OS" != "Darwin" ] && [ "$DRY_RUN" -eq 0 ]; then
  run $SUDO apt-get update -y >/dev/null 2>&1 || say "WARN: apt-get update failed (continuing)"
  # Extractors the GitHub-binary path needs (zip assets: jless/procs/ast-grep; xz: watchexec).
  have unzip || $SUDO apt-get install -y unzip >/dev/null 2>&1 || say "WARN: could not install unzip (zip assets will fail)"
  have xz || $SUDO apt-get install -y xz-utils >/dev/null 2>&1 || say "WARN: could not install xz-utils (watchexec will fail)"
fi

for t in "${TOOLS[@]}"; do install_one "$t"; done

# Wire delta as git's pager (per-machine, idempotent) only if delta is available
# and the user hasn't already chosen a pager.
if have delta || [ "$DRY_RUN" -eq 1 ]; then
  current_pager="$(git config --global --get core.pager 2>/dev/null || true)"
  if [ -z "$current_pager" ] || [ "$current_pager" = "delta" ]; then
    run git config --global core.pager delta
    run git config --global interactive.diffFilter "delta --color-only"
    run git config --global delta.navigate true
    say ""
    say "Wired delta as git core.pager (interactive diffFilter + navigate)."
  else
    say ""
    say "Left existing git core.pager='$current_pager' untouched (set it to 'delta' manually if you want it)."
  fi
fi

# ---- summary -------------------------------------------------------------
say ""
say "================ summary ================"
[ ${#PRESENT[@]} -gt 0 ] && say "already present : ${PRESENT[*]}"
if [ "$DRY_RUN" -eq 1 ]; then
  [ ${#PLANNED[@]} -gt 0 ] && say "would install  : ${PLANNED[*]}"
else
  [ ${#INSTALLED[@]} -gt 0 ] && say "installed      : ${INSTALLED[*]}"
  [ ${#FAILED[@]} -gt 0 ] && say "FAILED         : ${FAILED[*]}"
fi
say ""
say "Note: ast-grep is invoked as 'ast-grep' (its 'sg' alias collides with"
say "util-linux). To shorten it, add to your shell rc:  alias astg='ast-grep'"
if [ "$BIN_DIR" = "${HOME}/.local/bin" ]; then
  say "Ensure '$BIN_DIR' is on PATH (add to ~/.bashrc / ~/.zshenv if missing)."
fi
[ ${#FAILED[@]} -eq 0 ]
