---
allowed-tools: Read, Write, Edit, Bash, Glob, Grep, Agent, Skill, AskUserQuestion, ScheduleWakeup, ToolSearch, mcp__plugin_mnemosyne_workspace__board_list, mcp__plugin_mnemosyne_workspace__board_place, mcp__plugin_mnemosyne_workspace__board_promote, mcp__plugin_mnemosyne_workspace__document_create, mcp__plugin_mnemosyne_workspace__document_get, mcp__plugin_mnemosyne_workspace__document_update, mcp__plugin_mnemosyne_workspace__document_comment, mcp__plugin_mnemosyne_workspace__document_tags_update, mcp__plugin_mnemosyne_workspace__document_list, mcp__plugin_mnemosyne_workspace__workspace_projects, mcp__plugin_mnemosyne_memory__memory_search, mcp__plugin_mnemosyne_memory__memory_list, mcp__plugin_mnemosyne_memory__memory_get, mcp__plugin_mnemosyne_memory__memory_store, mcp__plugin_mnemosyne_memory__memory_update
description: Autonomous wave-by-wave loop — run a declared /goal to merged completion on main (and a v0.x.y tag if the goal asks)
argument-hint: [until /goal <objective or Glen board issue/module> [and release v0.x.y]]
model: inherit
---

Load the `auto-mode` skill at `.claude/skills/auto-mode/SKILL.md` and follow it exactly. Read `.claude/skills/auto-mode/references/loop-procedures.md` before the first wave.

Auto-mode invocation: $ARGUMENTS

Procedure:

1. If `$ARGUMENTS` carries an inline goal (`until /goal ...`, `/goal ...`, a board issue (`W<n>` label or document id), or an objective phrase), persist it to `.claude/auto-mode/state.md` as a `/goal` write first (same parsing as `.claude/commands/goal.md`), noting whether it is a release goal (explicit `release` / `tag v0.x.y` phrasing only).
2. Read `.claude/auto-mode/state.md`. If no goal is set anywhere, ask the user to declare one via `/goal` and stop — do not start a loop without a finish line.
3. Every goal targets `main`; there is no base branch to ask about and no staging or promotion step.
4. Otherwise begin or resume the autonomous loop against the active goal per the skill: recall Mnemosyne project `glen`, resolve waves (Glen board children first — Mnemosyne workspace project Glen, `projectId` `ff92abe1-da63-402d-a4f1-c8690e471be1` — then plan fallback), then run the wave pipeline — each wave branches off `main`, PRs into `main`, passes CI, gets a `memory_store` referencing the PR, and squash-merges — without a user gate between waves. A release goal ends with the `release` skill (version-bump PR, then an annotated `v0.x.y` tag on `main`).
