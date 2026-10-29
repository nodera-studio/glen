---
allowed-tools: Read, Write, Edit, Bash, Glob, Grep, Agent
description: Fast path for small, low-risk changes — fused implement + one review, no planning/advisor/conformance gates
argument-hint: [small task description]
model: inherit
---

Read the orchestrator prompt at `.claude/agents/implementation/orchestrator-quick.md` and follow it exactly.

The user's task: $ARGUMENTS

If no arguments were provided (empty or blank), ask the user what small change they want before proceeding.
