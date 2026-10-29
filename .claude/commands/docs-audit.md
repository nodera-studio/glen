---
allowed-tools: Read, Write, Edit, Bash, Glob, Grep, Agent, Skill
description: Documentation accuracy audit -- cross-references docs against actual code (2 Claude + 1 Codex)
argument-hint: (no arguments)
model: inherit
---

Read `.claude/agents/codebase/docs-audit/orchestrator.md` and follow its instructions exactly.

Run the full documentation accuracy audit: dispatch all 3 agents (2 Claude domain auditors +
1 Codex stale reference scanner), synthesize findings, and produce the audit report at
`.claude/reviews/docs-audit-{date}.md`.
