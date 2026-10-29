#!/usr/bin/env bash
# PreCompact + SessionStart(resume): re-inject the non-negotiable invariants so
# long autonomous/agentic runs survive compaction. Payload deliberately < ~500 tokens.
set -uo pipefail

cat <<'EOF'
Non-negotiable project invariants (re-injected post-compaction):
1. [Add your project's non-negotiable invariants here — e.g. a data-access pattern that must never be bypassed, a testing policy, a git-hygiene rule]
2. [e.g. "Tests are source of truth: never adjust assertions to pass; failing test = report + fix code or flag."]
3. [e.g. "Migrations/schema changes must stay paired across every generated artifact — check for collisions before landing."]
4. [e.g. "Rules files are paths:-scoped (lazy). The behavioral preamble (rules/behavioral-preamble.md) is the only intentional always-loaded rule."]
Tool routing (hard rules — see your project's CLAUDE.md index + per-tool skills for the HOW):
- Web search/fetch policy: [state whether native WebSearch/WebFetch are denied and what replaces them].
- Secrets: [state your managed secret path] — never echo secret values.
- Mutators stay prompt-gated (destructive infra ops, external-API writes, `gh pr merge`) — don't assume an allow.
EOF
exit 0
