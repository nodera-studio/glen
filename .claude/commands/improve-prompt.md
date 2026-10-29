---
allowed-tools: Read, Write, Edit, Bash, Glob, Grep
model: inherit
description: Optimize a prompt or agent file using Anthropic's prompt engineering best practices
---

Read `.claude/prompts/prompt-engineering-claude.md` and follow its transformation rules.

The user wants to improve: $ARGUMENTS

If no arguments provided, ask the user which prompt or agent file to optimize.

For agent files (.claude/agents/\*_/_.md): preserve the YAML frontmatter (name, description, tools, model, color) and optimize the prompt body.

Show a before/after diff summary of key changes before applying edits.
