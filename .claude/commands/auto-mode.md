---
allowed-tools: Read, Write, Edit, Bash, Glob, Grep, Agent, Skill, AskUserQuestion, ScheduleWakeup, mcp__linear-server__get_issue, mcp__linear-server__list_issues, mcp__linear-server__save_issue, mcp__linear-server__save_comment, mcp__linear-server__list_issue_statuses
description: Autonomous wave-by-wave loop — run a declared /goal to merged completion on main or staging
argument-hint: [until /goal ship NOD-XXX on the main|staging branch]
model: inherit
---

Load the `auto-mode` skill at `.claude/skills/auto-mode/SKILL.md` and follow it exactly.

Auto-mode invocation: $ARGUMENTS

Procedure:

1. If `$ARGUMENTS` carries an inline goal (`until /goal ...`, `/goal ...`, a bare `NOD-XXX`, or an objective phrase), persist it to `.claude/auto-mode/state.md` as a `/goal` write first (same parsing as `.claude/commands/goal.md`), noting whether it's a feature goal (base `staging`) or a `staging → main` promotion (`to prod` / `on main`).
2. Read `.claude/auto-mode/state.md`. If no goal is set anywhere, ask the user to declare one via `/goal` and stop — do not start a loop without a finish line.
3. Default the base to `staging` (feature goal) when the goal doesn't say otherwise — don't ask. Only a goal phrased `to prod` / `promote to main` is a promotion goal.
4. Otherwise begin or resume the autonomous loop against the active goal per the skill: resolve waves (Linear sub-issues first, plan fallback), then run the wave pipeline — each wave branches off `staging` and PRs into `staging` — without a user gate between waves. There is no integration branch; a goal that targets prod ends with ONE `staging → main` promotion (merge commit) per the `promote-to-prod` skill.
