---
allowed-tools: Read, Write, Edit, Bash, Glob, Grep, Agent, Skill
description: Tech debt audit -- pattern consistency, code complexity, TODO inventory, library currency (3 Claude + 1 Codex)
argument-hint: (no arguments)
model: inherit
---

Read `.claude/agents/codebase/tech-debt-audit/orchestrator.md` and follow its instructions exactly.

Run the full tech debt audit: dispatch all 4 agents (3 Claude + 1 Codex covering pattern
consistency, code complexity, TODO/marker inventory, and library currency), synthesize
findings, update the tech debt registry at `.claude/tech-debt/`, and produce the audit
report at `.claude/reviews/tech-debt-audit-{date}.md`.

**Verification (overrides the orchestrator's single-pass validation).** Every finding of every
severity, LOW included, gets two independent verifier lenses before it enters the report:
REPRODUCE (run it, or cite the exact `path:line`) and REFUTE (look for the guard, test or
convention that makes it a non-issue). Keep a finding only when REPRODUCE confirms it and REFUTE
fails to refute it, and record the kill rate per lens in the report. To run the arms and the
verifiers as a workflow, see `.claude/docs/Workflow-Fanout.md`.
