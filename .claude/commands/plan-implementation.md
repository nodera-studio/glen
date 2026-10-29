---
allowed-tools: Read, Write, Bash, Grep, Glob, Agent, WebFetch, WebSearch, mcp__context7__resolve-library-id, mcp__context7__query-docs
description: Explore, research, and architect a detailed implementation plan before coding
argument-hint: [task description]
model: inherit
---

Read the orchestrator prompt at `.claude/agents/plan/orchestrator.md` and follow the planning pipeline described there exactly.

The user's task: $ARGUMENTS

If no arguments were provided (empty or blank), ask the user what they want to plan before proceeding.
