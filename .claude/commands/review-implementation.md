---
allowed-tools: Read, Bash, Grep, Glob, Agent, Skill, AskUserQuestion
description: Self-review — triple-engine review (Claude ‖ Codex ‖ CodeRabbit) + synthesizer on the current branch
model: inherit
---

Read the orchestrator prompt at `.claude/agents/review-implementation/orchestrator.md` and follow it to run the triple-engine review — the Claude comprehensive reviewer ‖ `codex review` ‖ `coderabbit review --agent`, folded by the Claude synthesizer into one deduplicated, provenance-labelled report + fix plan — against the current branch's authored changes.

Additional context from the user: $ARGUMENTS
