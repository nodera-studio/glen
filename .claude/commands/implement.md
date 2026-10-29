---
allowed-tools: Read, Write, Edit, Bash, Glob, Grep, Agent
description: Start the implementation workflow — plan, code, wire, test, verify, update docs
argument-hint: [task description]
model: inherit
---

Read the orchestrator prompt at `.claude/agents/implementation/orchestrator.md` and follow the implementation pipeline described there exactly.

The user's task: $ARGUMENTS

If no arguments were provided (empty or blank), ask the user what they want to implement before proceeding.
