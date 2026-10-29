---
allowed-tools: Read, Agent, Bash
description: Dispatch a task to OpenAI Codex via the codex:codex-rescue plugin agent
argument-hint: [prompt for codex]
model: inherit
---

Read `.claude/skills/codex/SKILL.md` and follow its instructions exactly to
dispatch the user's request to the `codex:codex-rescue` agent.

User's prompt for Codex: $ARGUMENTS

If `$ARGUMENTS` is empty, ask the user what they want Codex to do before
dispatching anything.
