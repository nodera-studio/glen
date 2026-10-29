#!/usr/bin/env bash
# PreToolUse(Bash) — steer off native shell tools onto Claude's structured tools.
# Reads the hook JSON on stdin; emits a deny JSON on exit 0 (block-and-instruct — the
# updatedInput rewrite is documented but unreliable, so we deny + name the replacement).
#
# Scope (deliberately narrow — over-blocking traps the agent in retry loops):
#   - `sed -i` editing a file        -> Edit tool / LSP rename
#   - `cat`/`head`/`tail` of a CODE file (not heredocs, not logs) -> Read tool
#   - shell reads of `.env`/secret files (not `.env.example`)     -> secrets MCP
#   - `curl`/`wget` to an EXTERNAL url -> Exa MCP (web) / a service SDK-CLI (APIs).
#     Loopback stays allowed (local API, MCP servers, health checks).
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

# split on pipes and ; & && || so each segment is inspected on its own
# (|| [ -n "$seg" ] handles the final newline-less segment; tr is BSD/GNU-portable)
while IFS= read -r seg || [ -n "$seg" ]; do
  [ -z "$seg" ] && continue
  base=$(printf '%s' "$seg" | sed -E 's/^[[:space:]]*//; s/^[A-Za-z_][A-Za-z0-9_]*=[^ ]* //' | awk '{print $1}')
  base=$(basename "$base" 2>/dev/null)

  # shell reads of real secret files (not .env.example) -> secrets MCP
  if printf '%s' "$seg" | grep -qE '\.env(\.[a-z._-]+)?' \
     && ! printf '%s' "$seg" | grep -qE '\.env(\.[a-z._-]*)?\.example' \
     && printf '%s' "$base" | grep -qE '^(cat|less|more|head|tail|grep|rg|bat|xxd|od|strings|awk|sed)$'; then
    deny "Don't read .env / secret files from the shell. Use the secrets MCP (mcp__secrets__*) — secrets must never enter context. (.env.example templates are fine via the Read tool.)"
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
         && printf '%s' "$seg" | grep -qE '\.(ts|tsx|js|jsx|mjs|cjs|json|md|mdx|css|scss|sql|ya?ml|toml|env)([[:space:]]|$|"|'"'"')'; then
        deny "Don't read files with $base. Use the Read tool — cacheable, structured, supports offset/limit."
      fi
      ;;
    curl|wget)
      # Service-specific redirects fire FIRST (more useful reason than the generic deny below).
      # These name the ONE replacement tool — no routing index duplicated in the hook.
      if printf '%s' "$seg" | grep -qiE 'https?://([a-z0-9.-]+\.)?cloudflare\.com([:/ ]|$|"|'"'"')'; then
        deny "Don't curl Cloudflare's API — use the wrangler CLI (deploy / R2 / D1 writes, headless) or the Cloudflare MCP (read-only account queries). See the /wrangler skill."
      fi
      if printf '%s' "$seg" | grep -qiE 'https?://([a-z0-9.-]+\.)?sentry\.io([:/ ]|$|"|'"'"')'; then
        deny "Don't curl Sentry's API — use sentry-cli (releases / sourcemaps) or the Sentry MCP (issue lookup / Seer). See the /sentry-cli skill."
      fi
      if printf '%s' "$seg" | grep -qiE 'https?://([a-z0-9.-]+\.)?linear\.app([:/ ]|$|"|'"'"')'; then
        deny "Don't curl Linear's API — use the Linear connector (mcp__*Linear* tools). See the /linear skill."
      fi
      # Loopback stays allowed. External http(s) fetches are denied: route web
      # CONTENT to the Exa MCP, and known APIs to their SDK/CLI — WebFetch is
      # off by design, and ad-hoc scraping bypasses the stack. Extend
      # ALLOW_HOSTS for any new endpoint you genuinely need (the deliberate
      # "deny-but-whitelist" knob). curl/wget with no http url (--version,
      # file://, pipes) is fine.
      # Add your own trusted remote hosts here if needed (e.g. your own infra
      # boxes, self-hosted services) — only loopback ships by default.
      ALLOW_HOSTS='localhost|127\.0\.0\.1|0\.0\.0\.0|::1'
      if printf '%s' "$seg" | grep -qiE 'https?://' \
         && ! printf '%s' "$seg" | grep -qiE "https?://\[?($ALLOW_HOSTS)\]?([:/ ]|\$|\"|'|$)"; then
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
  esac
done < <(printf '%s\n' "$CMD" | tr '|;&' '\n')
exit 0
