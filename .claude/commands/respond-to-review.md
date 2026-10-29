---
allowed-tools: Read, Write, Edit, Bash, Grep, Glob, Agent, WebFetch, WebSearch
description: Classify PR review comments, verify claims, draft responses, apply fixes
argument-hint: [paste review comments after invoking]
model: inherit
---

Read the agent prompt at `.claude/agents/respond-to-review.md` and follow its instructions exactly.

The user will paste PR review comments from Bitbucket after this command runs.
Ask them to paste the comments if they haven't already.

User's input: $ARGUMENTS
