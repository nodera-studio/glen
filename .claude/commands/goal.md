---
allowed-tools: Read, Write, Edit
description: Declare or update the auto-mode goal (objective + base branch) for the autonomous loop
argument-hint: ship NOD-XXX on the main|staging branch
model: inherit
---

Declare the auto-mode goal. This command only records intent — it does not start the loop. `/auto-mode` reads what you set here.

Goal: $ARGUMENTS

Procedure:

1. If `$ARGUMENTS` is empty, ask the user for the objective (and which base branch — `main` or `staging`) and stop.
2. Parse the objective and whether this is a promotion. Treat ONLY explicit promotion phrasing — `to prod`, `promote staging to main`, `on the main branch` — as a `staging → main` promotion goal; everything else is a feature goal with base `staging`. A bare `on main` is ambiguous (it can mean "fix the bug on main"): ask the user to confirm rather than assuming a promotion. Default to `staging` when unstated — `main` work is always an explicit, confirmed promotion.
3. Read `.claude/auto-mode/state.md` if it exists; otherwise create it from the state-file template in `.claude/skills/auto-mode/SKILL.md`.
4. Write/overwrite the `## Goal` section with the objective, fill `Base branch` under `## Resolved goal` if known, and set `## Status` to `goal-set`. Preserve any existing wave ledger only if it still matches this goal; otherwise reset the ledger.
5. Confirm in one line what the goal and base branch are, and that the user can now run `/auto-mode` to start.
