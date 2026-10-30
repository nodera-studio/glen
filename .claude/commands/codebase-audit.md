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

Read `.claude/agents/codebase/orchestrator.md` and follow it: it holds the scope modes,
the inline scan recipes (tech-debt-light, cli-surface-light, docs-light, all cargo/rg),
and the synthesis steps.

**Quick mode:** dispatch `.claude/agents/codebase/dependency-audit.md` via the Agent
tool; run the three inline scans yourself while it works; then synthesize.

**Deep mode:** dispatch the four tier-2 audits sequentially (each is heavy —
sequential avoids context overload), each via the Agent tool reading its command
file: `security-audit`, `error-audit`, `docs-audit`, `tech-debt-audit`
(`.claude/commands/{name}.md`). Then synthesize and update `.claude/tech-debt/` with
new HIGH/CRITICAL findings.

<rules>
- Always produce the synthesis summary — raw agent outputs are too long for the
  user to parse.
- This is an audit: report findings only. Do not fix issues or commit changes.
</rules>

Scope: $ARGUMENTS
