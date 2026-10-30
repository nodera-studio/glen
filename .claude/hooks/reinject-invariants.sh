#!/usr/bin/env bash
# PreCompact + SessionStart(resume): re-inject the non-negotiable invariants so
# long autonomous/agentic runs survive compaction. Payload deliberately < ~500 tokens.
set -uo pipefail

cat <<'EOF'
Non-negotiable glen invariants (re-injected post-compaction; full guide: AGENTS.md, .claude/CLAUDE.md):
1. Never write to an omp session's .jsonl and never take its .jsonl.lock.os append gate. omp owns them;
   glen reads, and sends through omp (RPC or the glen bridge extension) so omp does the writing.
2. Parse omp formats leniently: skip unknown entry types and fields, never fail on them.
3. Tests that need omp data use tests/fixtures/, never a live ~/.omp.
4. ../metis/references/oh-my-pi is a read-only reference clone (omp v18.14.0): read it, never edit or push it.
5. Trunk is main: branch off main, PR into main, squash-merge, no direct pushes. No staging or release
   branch; releases are v0.x.y tags on main (release skill). Store a Mnemosyne memory referencing the
   PR before `gh pr merge`.
6. Tests are the source of truth: never adjust assertions to pass; a failing test means fix the code or flag it.
7. Rules files are paths:-scoped; rules/behavioral-preamble.md is the only always-loaded rule.
8. If .claude/auto-mode/state.md exists with Status planning/in-progress/landing/blocked, an /auto-mode
   run is active: re-read it and .claude/skills/auto-mode/SKILL.md before the next step. Waves PR into
   main; the tracker is the Mnemosyne board of project Glen (ff92abe1-da63-402d-a4f1-c8690e471be1).
Tool routing (hard rules):
- Native WebSearch/WebFetch are denied: web content goes to the Exa MCP, library docs to the ctx7 CLI (Context7).
- Memory, issues and secrets are Mnemosyne (mcp__plugin_mnemosyne_*), project "glen". Issues are on the
  Mnemosyne board, not Linear or GitHub Issues. Never echo a secret value.
- Codex only through codex:codex-rescue, codex-companion.mjs or `codex review`; never `codex exec`.
- Mutators stay prompt-gated (`gh pr merge`, pushing a release tag, external-API writes); don't assume an allow.
EOF
exit 0
