---
allowed-tools: Read, Write, Edit
description: Declare or update the auto-mode goal (objective, and whether it ends in a release tag) for the autonomous loop
argument-hint: <objective, Glen board issue/module, or plan path> [and release v0.x.y]
model: inherit
---

Declare the auto-mode goal. This command only records intent — it does not start the loop. `/auto-mode` reads what you set here.

Goal: $ARGUMENTS

Procedure:

1. If `$ARGUMENTS` is empty, ask the user for the objective and stop.
2. Parse the objective. Every goal targets `main` — glen's only long-lived branch; there is no base branch to choose and no staging or promotion step. Note what the goal points at: a Glen board module or issue (key, title or id), a plan under `.claude/plans/`, or a freeform objective. Treat the goal as a **release goal** only on explicit release phrasing — `release`, `tag v0.x.y`, `ship version 0.x.y`, `cut a release` — and record the version if one is named (otherwise "next patch/minor, decided at release time"). Anything else is a feature goal with no release.
3. Read `.claude/auto-mode/state.md` if it exists; otherwise create it (and `.claude/auto-mode/`) from the state-file template in `.claude/skills/auto-mode/references/loop-procedures.md`. The file is git-ignored; never stage it.
4. Write/overwrite the `## Goal` section with the objective, fill `Goal module / issue` and `Release` under `## Resolved goal` where known, and set `## Status` to `goal-set`. Preserve any existing wave ledger only if it still matches this goal; otherwise reset the ledger.
5. Confirm in one line what the goal is and whether it ends in a release, and that the user can now run `/auto-mode` to start.
