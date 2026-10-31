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

**Verification (overrides the orchestrator's single-pass validation).** Every finding of every
severity, LOW included, gets two independent verifier lenses before it enters the report:
REPRODUCE (run it, or cite the exact `path:line`) and REFUTE (look for the guard, test or
convention that makes it a non-issue). Keep a finding only when REPRODUCE confirms it and REFUTE
fails to refute it, and record the kill rate per lens in the report. To run the arms and the
verifiers as a workflow, see `.claude/docs/Workflow-Fanout.md`.
