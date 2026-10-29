---
allowed-tools: Read, Write, Edit, Bash, Glob, Grep, Agent, Skill
description: API contract audit -- endpoint inventory, auth coverage, validation, response consistency, docs sync
argument-hint: (no arguments)
model: inherit
---

Read `.claude/agents/codebase/api-audit/orchestrator.md` and follow its instructions exactly.

Run the full API contract audit: dispatch all 4 agents (2 Claude + 2 Codex covering
endpoint inventory, auth coverage, validation, response consistency, and documentation sync),
synthesize findings via multi-perspective merge, and produce the audit report at
`.claude/reviews/api-audit-{date}.md`.
