---
allowed-tools: Read, Write, Edit, Bash, Glob, Grep, Agent, Skill
description: Run codebase health check — quick (default) or deep (full tier 2 audits)
argument-hint: [quick|deep]
model: inherit
---

<role>
Codebase audit orchestrator. Two tiers: a fast "quick" scan (minutes) or a
"deep" audit dispatching the full-scope specialist suites. Either way you
produce a unified synthesis the user can act on.
</role>

Mode from `$ARGUMENTS`: none/`quick`/`light`/`check` → **quick**; `deep`/`full`/`all`
→ **deep**. Announce the mode before starting.

Scan recipes and report templates live in `.claude/docs/CodebaseAuditScans.md` —
read it when you start.

**Quick mode:** dispatch `codebase/dependency-audit.md` and `codebase/dead-code.md`
via the Agent tool in parallel; run the three inline scans (tech-debt-light,
api-surface-light, docs-light) yourself from the scans doc while they work; then
synthesize with the quick-mode template.

**Deep mode:** dispatch the five tier-2 audits sequentially (each is heavy —
sequential avoids context overload), each via the Agent tool reading its command
file: `security-audit`, `error-audit`, `api-audit`, `docs-audit`,
`tech-debt-audit` (`.claude/commands/{name}.md`). Then synthesize with the
deep-mode template and update `.claude/tech-debt/` with new HIGH/CRITICAL
findings.

<rules>
- Always produce the synthesis summary — raw agent outputs are too long for the
  user to parse.
- This is an audit: report findings only. Do not fix issues or commit changes.
</rules>

Scope: $ARGUMENTS
