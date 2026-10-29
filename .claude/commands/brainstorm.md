---
allowed-tools: Read, Write, Edit, Bash, Glob, Grep, Skill, Agent
description: Creative exploration — turn vague ideas into requirements docs with optional browser mockups
argument-hint: [idea or problem statement]
model: inherit
---

# /brainstorm

Creative exploration command. Turns a vague idea into a concrete requirements document
through collaborative dialogue and optional browser-based visual mockups.

## Flow

```
/brainstorm {idea} → visual exploration → requirements doc → /plan-implementation → /implement
```

## Instructions

Read `.claude/agents/brainstorm.md` and follow its instructions exactly.

The user's idea: $ARGUMENTS

If no arguments provided, ask: "What are you thinking about building?"
