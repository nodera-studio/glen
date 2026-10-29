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
