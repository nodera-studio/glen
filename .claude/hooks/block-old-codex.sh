#!/usr/bin/env bash
set -euo pipefail

# Blocks legacy Codex transports. The canonical Codex dispatch path is the
# openai-codex plugin's subagent (`Agent(subagent_type: 'codex:codex-rescue')`).
#
# Blocks:
#   1. Skill calls to `skill-codex:codex` (the redundant skill wrapper).
#   2. Direct `codex exec ...` Bash invocations (legacy direct CLI).
#
# Allows:
#   - `codex login`, `codex --version`, `codex app-server` (auth / version probes).
#   - Any command that runs through `codex-companion.mjs` (the plugin's broker).

input=$(cat)
tool=$(echo "$input" | jq -r '.tool_name // ""')

case "$tool" in
  Skill)
    skill=$(echo "$input" | jq -r '.tool_input.skill // ""')
    if [ "$skill" = "skill-codex:codex" ] || [ "$skill" = "plugin:skill-codex:codex" ]; then
      cat >&2 <<'EOF'
BLOCKED: `Skill(skill-codex:codex)` is the dropped legacy wrapper.

Use the openai-codex plugin's subagent instead:

  Agent({
    subagent_type: 'codex:codex-rescue',
    description: '<short>',
    prompt: '<your task — verbatim, as if you were sending it to Codex>',
  })

The subagent forwards through the plugin's shared session runtime
(unix socket at cxc-*/broker.sock), reuses your ChatGPT login, and
handles concurrency correctly.
EOF
      exit 2
    fi
    exit 0
    ;;

  Bash)
    cmd=$(echo "$input" | jq -r '.tool_input.command // ""')

    # Allow anything that runs through the plugin's companion broker —
    # it internally exec's the codex binary and that's the canonical path.
    if echo "$cmd" | grep -qE 'codex-companion\.mjs'; then
      exit 0
    fi

    # Block raw `codex exec ...` invocations (the legacy direct-CLI path).
    # `codex login`, `codex --version`, `codex app-server` etc. stay allowed.
    if echo "$cmd" | grep -qE '(^|[ |&;`(])codex[[:space:]]+exec([[:space:]]|$)'; then
      cat >&2 <<'EOF'
BLOCKED: Direct `codex exec ...` is the dropped legacy invocation.

Use the openai-codex plugin's subagent instead:

  Agent({
    subagent_type: 'codex:codex-rescue',
    description: '<short>',
    prompt: '<your task — verbatim, as if you were sending it to Codex>',
  })

The subagent forwards through the plugin's companion broker
(~/.claude/plugins/cache/openai-codex/codex/1.0.4/scripts/codex-companion.mjs),
which handles auth + concurrency under the shared session runtime.

If you genuinely need to verify the binary is installed or log in,
`codex --version` / `codex login` are still allowed.
EOF
      exit 2
    fi

    exit 0
    ;;

  *)
    exit 0
    ;;
esac
