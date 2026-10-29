---
allowed-tools: Read, Write, Edit, Bash, Glob, Grep, Agent, Skill
description: Exhaustive error path audit -- 6 dual-engine agents (backend, frontend, queue)
argument-hint: (no arguments)
model: inherit
---

Read `.claude/agents/codebase/error-audit/orchestrator.md` and follow its instructions exactly.

Run the full error path audit: dispatch all 6 agents (3 Claude + 3 Codex covering backend,
frontend, and queue error paths), synthesize findings via multi-perspective merge, validate
every finding, and produce the audit report at `.claude/reviews/error-audit-{date}.md`.
