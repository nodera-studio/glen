---
allowed-tools: Read, Write, Edit, Bash, Glob, Grep, Agent, Skill
description: OWASP ASVS Level 2 security audit -- 12 dual-engine agents with checklist perturbation
argument-hint: (no arguments)
model: inherit
---

Read `.claude/agents/codebase/security-audit/orchestrator.md` and follow its instructions exactly.

Run the full OWASP ASVS Level 2 audit: dispatch all 12 agents (5 Claude group agents +
5 Codex group agents + 2 checklist perturbation passes), synthesize findings, validate
every FAIL, and produce the compliance report.

**Verification (overrides the orchestrator's single-pass validation).** Every finding of every
severity, LOW included, gets two independent verifier lenses before it enters the report:
REPRODUCE (run it, or cite the exact `path:line`) and REFUTE (look for the guard, test or
convention that makes it a non-issue). Keep a finding only when REPRODUCE confirms it and REFUTE
fails to refute it, and record the kill rate per lens in the report. To run the arms and the
verifiers as a workflow, see `.claude/docs/Workflow-Fanout.md`.
