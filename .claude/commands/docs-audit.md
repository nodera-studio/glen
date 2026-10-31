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

**Verification (overrides the orchestrator's single-pass validation).** Every finding of every
severity, LOW included, gets two independent verifier lenses before it enters the report:
REPRODUCE (run it, or cite the exact `path:line`) and REFUTE (look for the guard, test or
convention that makes it a non-issue). Keep a finding only when REPRODUCE confirms it and REFUTE
fails to refute it, and record the kill rate per lens in the report. To run the arms and the
verifiers as a workflow, see `.claude/docs/Workflow-Fanout.md`.
