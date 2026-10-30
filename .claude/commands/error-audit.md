---
allowed-tools: Read, Write, Edit, Bash, Glob, Grep, Agent, Skill
description: Exhaustive error path audit of glen's crate -- 2 dual-engine agents (Claude ‖ Codex)
argument-hint: (no arguments)
model: inherit
---

Read `.claude/agents/codebase/error-audit/orchestrator.md` and follow its instructions exactly.

Run the full error path audit: dispatch both agents (Claude `backend.md` ‖ Codex via
`codex:codex-rescue`, covering the crate's CLI entry, omp file parsing, omp RPC/bridge I/O
and output), synthesize findings via multi-perspective merge, validate every finding, and
produce the audit report at `.claude/reviews/error-audit-{date}.md`.
