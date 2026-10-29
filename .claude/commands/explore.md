---
allowed-tools: Agent, Read, Glob, Grep, Bash
description: Deep read-only codebase exploration — map all code relevant to a task
argument-hint: [task or area to explore]
model: inherit
---

Dispatch the codebase explorer to thoroughly map all code relevant to the task below, then
return its report.

Use the Agent tool with `subagent_type: plan-explorer` (the agent defined at
`.claude/agents/plan/explorer.md`). Pass it the task verbatim and relay its findings — the
explorer is read-only (it locates and maps code; it does not edit).

The task to explore: $ARGUMENTS

If no arguments were provided (empty or blank), ask the user what they want to explore before
dispatching.

## Observation

This command is stateless: the explorer's report is returned inline and reviewed by the caller
on every run. There is no telemetry to capture and no self-amending state — observation is the
caller reading the returned map each invocation.

## Feedback

To correct a poor exploration, refine the `$ARGUMENTS` scope (narrow the area or name the
subsystem) and re-run. The command does not learn between runs, so there is no stored feedback
loop to tune.

## Rollback

`git rm .claude/commands/explore.md` removes the command; the wrapped `plan-explorer` agent at
`.claude/agents/plan/explorer.md` is independent and unaffected.
