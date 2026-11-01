#!/usr/bin/env bash
# PreToolUse(Bash) — steer off native shell tools onto Claude's structured tools.
# Reads the hook JSON on stdin; emits a deny JSON on exit 0 (block-and-instruct — the
# updatedInput rewrite is documented but unreliable, so we deny + name the replacement).
#
# Scope (deliberately narrow — over-blocking traps the agent in retry loops):
#   - `sed -i` editing a file        -> Edit tool / LSP rename
#   - `cat`/`head`/`tail` of a CODE file (not heredocs, not logs) -> Read tool
#   - shell reads of `.env`/secret files (not `.env.example`)     -> Mnemosyne secrets MCP
#   - `curl`/`wget` to an EXTERNAL url -> Exa MCP (web) / a service SDK-CLI (APIs).
#     Loopback stays allowed (local API, MCP servers, health checks).
#   - `omp` with a profile other than glen-harness (`--profile`, OMP_PROFILE, PI_PROFILE), and
#     `mkdir`/`cp`/`mv`/`rsync`/`ln` into another ~/.omp/profiles/<name> -> HOME="$(mktemp -d)"
#   - recursive `rm` of glen- scratch under $TMPDIR, /tmp or /var/folders -> scripts/clean-scratch.sh
# Does NOT touch grep/rg (agentic search is good in-loop — routed via CLAUDE.md)
# or codex (block-old-codex.sh). Fails open on any error. Project-specific
# steering (e.g. a dev-tool wrapper hook) can be layered in separately — see
# hooks/README-optional-hooks.md.
set -uo pipefail
command -v jq >/dev/null 2>&1 || exit 0
INPUT=$(cat 2>/dev/null) || exit 0
CMD=$(printf '%s' "$INPUT" | jq -r '.tool_input.command // empty' 2>/dev/null) || exit 0
[ -z "$CMD" ] && exit 0

deny() {
  jq -n --arg r "$1" '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:$r}}' 2>/dev/null
  exit 0
}

# omp probes (s175): omp joins PI_CONFIG_DIR onto $HOME, and a named profile ignores
# PI_CODING_AGENT_DIR, so the only isolation is a throwaway HOME or the approved profile.
OMP_OK_PROFILE='glen-harness'
# shellcheck disable=SC2016 # the $(mktemp -d) and $HOME are literal advice text
OMP_PROFILE_REASON='omp probes use a throwaway home: HOME="$(mktemp -d)" omp … (omp joins PI_CONFIG_DIR onto $HOME, so an absolute PI_CONFIG_DIR still writes under ~), or the approved glen-harness profile. Never create another ~/.omp/profiles/<name> (owner rule).'
# a profile name with surrounding quotes (and a subshell's closing paren) removed
unquote() { printf '%s' "$1" | tr -d "\"')"; }
REAL_HOME="${HOME:-}"
[ -n "$REAL_HOME" ] || REAL_HOME=/nonexistent-home
# the command word of a segment behind `(` and the wrappers env (VAR=v, -i, -u N), timeout
# (-s S, -k K, the duration), nice (-n N), command, nohup, exec, time, builtin
cmd_word() {
  printf '%s' "$1" | awk '{
    i = 1
    while (i <= NF) {
      w = $i; sub(/^\(+/, "", w)
      if (w == "" || w ~ /^[A-Za-z_][A-Za-z0-9_]*=/) { i++; continue }
      if (w == "env") { i++; while (i <= NF && $i ~ /^-/) { if ($i == "-u") i++; i++ }; continue }
      if (w == "timeout") { i++; while (i <= NF && $i ~ /^-/) { if ($i == "-s" || $i == "-k") i++; i++ }; i++; continue }
      if (w == "nice") { i++; if ($i == "-n") i += 2; else if ($i ~ /^-/) i++; continue }
      if (w == "command" || w == "nohup" || w == "exec" || w == "time" || w == "builtin") { i++; while (i <= NF && $i ~ /^-/) i++; continue }
      print w; exit
    }
  }'
}
# omp_profile_check SEG: deny a --profile in SEG, or OMP_PROFILE= / PI_PROFILE= anywhere in the
# command (an `export` earlier in the chain counts), other than glen-harness. Empty = default.
omp_profile_check() {
  local names name
  names=$( { printf '%s' "$1" | grep -oE -- '--profile[= ]+[^[:space:]]+' | sed -E 's/^--profile[= ]+//'
             printf '%s' "$CMD" | grep -oE '(^|[^A-Za-z0-9_])(OMP|PI)_PROFILE=[^[:space:];&|]*' | sed -E 's/^.*_PROFILE=//'; } 2>/dev/null)
  while IFS= read -r name; do
    name=$(unquote "$name")
    if [ -n "$name" ] && [ "$name" != "$OMP_OK_PROFILE" ]; then
      deny "$OMP_PROFILE_REASON"
    fi
  done <<< "$names"
}

# split on pipes and ; & && || so each segment is inspected on its own
# (|| [ -n "$seg" ] handles the final newline-less segment; tr is BSD/GNU-portable)
while IFS= read -r seg || [ -n "$seg" ]; do
  [ -z "$seg" ] && continue
  base=$(printf '%s' "$seg" | sed -E 's/^[[:space:]]*//; s/^([A-Za-z_][A-Za-z0-9_]*=[^ ]* +)+//' | awk '{print $1}')
  base=$(basename "$base" 2>/dev/null)

  # omp behind a wrapper (timeout, env, a subshell), or inside `sh|bash|zsh -c '…'`
  inner=$(basename "$(cmd_word "$seg")" 2>/dev/null)
  if [ "$inner" = omp ] || { case "$inner" in sh|bash|zsh|dash) true ;; *) false ;; esac \
       && printf '%s' "$seg" | grep -qE "(^|[[:space:](\"'/])omp([[:space:]\"']|\$)"; }; then
    omp_profile_check "$seg"
  fi

  # shell reads of real secret files (not .env.example) -> secrets MCP
  if printf '%s' "$seg" | grep -qE '\.env(\.[a-z._-]+)?' \
     && ! printf '%s' "$seg" | grep -qE '\.env(\.[a-z._-]*)?\.example' \
     && printf '%s' "$base" | grep -qE '^(cat|less|more|head|tail|grep|rg|bat|xxd|od|strings|awk|sed)$'; then
    deny "Don't read .env / secret files from the shell. Use the Mnemosyne secrets MCP (mcp__plugin_mnemosyne_secrets__*, project \"glen\") — secrets must never enter context. (.env.example templates are fine via the Read tool.)"
  fi

  case "$base" in
    sed)
      if printf '%s' "$seg" | grep -qE '(^|[[:space:]])(-i|--in-place)'; then
        deny "Don't edit files with 'sed -i'. Use the Edit tool (deterministic + reviewable), or the LSP rename for symbol renames."
      fi
      ;;
    cat|head|tail)
      # allow heredocs (cat <<EOF) and non-code reads (logs etc.); block reads of code files
      if ! printf '%s' "$seg" | grep -qE '<<' \
         && printf '%s' "$seg" | grep -qE '\.(rs|ts|tsx|js|jsx|mjs|cjs|json|md|mdx|css|scss|sql|ya?ml|toml|env)([[:space:]]|$|"|'"'"')'; then
        deny "Don't read files with $base. Use the Read tool — cacheable, structured, supports offset/limit."
      fi
      ;;
    curl|wget)
      # Service-specific redirects go here when a project needs them (fire before the generic
      # deny below, naming the ONE replacement tool). glen uses none: it has no Cloudflare,
      # Sentry or Linear integration, so their upstream redirects were dropped.
      # Loopback stays allowed. External http(s) fetches are denied: route web
      # CONTENT to the Exa MCP, and known APIs to their SDK/CLI — WebFetch is
      # off by design, and ad-hoc scraping bypasses the stack. Extend
      # ALLOW_HOSTS for any new endpoint you genuinely need (the deliberate
      # "deny-but-whitelist" knob). curl/wget with no http url (--version,
      # file://, pipes) is fine.
      # Add your own trusted remote hosts here if needed (e.g. your own infra
      # boxes, self-hosted services). Allowed by default: loopback, plus glen's own
      # deployed site (W7) so live checks of status codes and headers can run.
      ALLOW_HOSTS='localhost|127\.0\.0\.1|0\.0\.0\.0|::1|glen\.nodera\.workers\.dev'
      # Every URL in the WHOLE command must be on the allow-list: one allowed URL must
      # not smuggle an external one through, and the segment splitter is not
      # quote-aware, so scan $CMD rather than $seg. Only a numeric port may follow the
      # host (`host:x@evil` is userinfo, not a port).
      bad_url=$(printf '%s' "$CMD" | grep -oiE "https?://[^[:space:]\"'\`]*" \
        | grep -viE "^https?://\[?($ALLOW_HOSTS)\]?(:[0-9]*)?([/?#]|\$)" | head -n 1)
      if [ -n "$bad_url" ]; then
        deny "Don't fetch external URLs with $base — WebFetch is off by design. For web content use the Exa MCP (web_search_exa / advanced search + crawl); for a known service use its SDK/CLI. Loopback is allowed — add a host to ALLOW_HOSTS in hooks/steer-bash.sh for anything else you genuinely need."
      fi
      ;;
    find)
      # File-SEARCH forms (-name/-path/-type f) -> rg --files / codebase MCP. OPERATIONAL forms
      # (-exec/-delete/-mtime/-newer/-size/-empty) are real work and stay allowed.
      if printf '%s' "$seg" | grep -qE '(^|[[:space:]])-(i?name|i?path|type)([[:space:]]|$)' \
         && ! printf '%s' "$seg" | grep -qE '(^|[[:space:]])-(exec(dir)?|delete|[mc]time|newer|size|empty)([[:space:]]|$)'; then
        deny "Don't search for files with 'find'. Use 'rg --files | rg <pattern>' (fast, respects .gitignore) for literal file lookups, or the codebase MCP for conceptual 'where does X live'. Operational find (-exec / -delete / -mtime …) is fine."
      fi
      ;;
    mkdir|cp|mv|rsync|ln)
      # only the real home's profiles (~, $HOME, ${HOME} or its literal path); a throwaway
      # HOME such as "$T/.omp/profiles/x" is what the advice asks for, so it stays allowed
      # shellcheck disable=SC2016 # $HOME / ${HOME} are matched literally in the command text
      names=$(printf '%s' "${seg//"$REAL_HOME"/\~}" \
        | grep -oE '(~|\$HOME|\$\{HOME\})"?/\.omp/profiles/[^/[:space:]]+' | sed -E 's#^.*\.omp/profiles/##' 2>/dev/null)
      while IFS= read -r name; do
        name=$(unquote "$name")
        if [ -n "$name" ] && [ "$name" != "$OMP_OK_PROFILE" ]; then
          deny "$OMP_PROFILE_REASON"
        fi
      done <<< "$names"
      ;;
    rm)
      # recursive deletes of glen scratch in the temp dirs -> the helper, which refuses anything
      # a worktree, process or omp engine run still uses
      # shellcheck disable=SC2016 # $TMPDIR / ${TMPDIR} are matched literally in the command text
      if printf '%s' "$seg" | grep -qE '(^|[[:space:]])(-[A-Za-z]*[rR][A-Za-z]*|--recursive)([[:space:]]|$)' \
         && printf '%s' "$seg" | grep -qE '(\$TMPDIR|\$\{TMPDIR[^}]*\}|/tmp/|/private/tmp/|/var/folders/|/private/var/folders/)[^[:space:]]*glen-'; then
        deny "Delete glen scratch with the helper: bash .claude/scripts/clean-scratch.sh [--dry-run] '<glen-name or glob>' (it refuses anything a worktree, process or omp engine run still uses). rm -rf stays denied."
      fi
      ;;
  esac
done < <(printf '%s\n' "$CMD" | tr '|;&' '\n')
exit 0
