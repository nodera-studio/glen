---
allowed-tools: Read, Bash, Grep, Glob, Agent, Skill, AskUserQuestion
description: Self-review — triple-engine review (Claude ‖ Codex ‖ omp) + synthesizer on the current branch
model: inherit
---

Read the orchestrator prompt at `.claude/agents/review-implementation/orchestrator.md` and follow it to run the triple-engine review — the Claude comprehensive reviewer ‖ `codex review` ‖ omp via `.claude/scripts/omp-engine.sh`, folded by the Claude synthesizer into one deduplicated, provenance-labelled report + fix plan — against the current branch's authored changes.

Additional context from the user: $ARGUMENTS
