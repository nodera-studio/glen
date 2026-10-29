---
allowed-tools: Read, Write, Bash, Agent
description: Toggle orchestrator mode — main session decomposes and dispatches only, never edits directly
argument-hint: [off]
model: inherit
---

# /orchestrate

Toggles orchestrator mode for the rest of this session.

## On (`/orchestrate`, no argument)

1. Touch the mode marker: `touch .claude/.orchestrator-mode`
2. Read `.claude/agents/orchestrator/orchestrator-mode.md` and adopt it as your standing
   instructions for every task until the mode is turned off.
3. Tell the user, in one or two sentences: orchestrator mode is on, you will decompose and
   dispatch every task rather than editing directly, and gaps (tasks with no matching
   harness agent) get logged for later review via `/audit-agent-gaps`.

## Off (`/orchestrate off`)

1. Remove the marker: `rm -f .claude/.orchestrator-mode`
2. Confirm orchestrator mode is off and you're back to normal direct-work behavior.

$ARGUMENTS
