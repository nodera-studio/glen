---
allowed-tools: Read, Glob, Grep, Bash, Agent, Skill, Write
description: Research any topic — Exa multi-agent fan-out (decompose → parallel sub-agents → merge), saves a cited report
argument-hint: [--quick|--deep] <topic or question>
model: inherit
---

# /research — Exa multi-agent fan-out

You are the research LEAD. Decompose the question, fan out to parallel research sub-agents
(each isolates its raw results in its own context), then merge into one cited report. Web
search is the **Exa MCP** — never the native WebSearch (denied), never Playwright (that's the
E2E-testing skill, not research). Context7 is available to you directly for version-correct
library docs.

Question: $ARGUMENTS

## Depth

- `--quick` — 1 sub-agent, the core question only. Fast.
- `--deep` — up to 5 sub-agents + a wider source net.
- _(no flag)_ — ~3 sub-agents.

## Procedure

1. **Decompose.** Break the question into 3–5 INDEPENDENT sub-questions with explicit,
   non-overlapping boundaries (so the sub-agents don't duplicate each other). State them to
   the user before fanning out.
2. **Fan out (parallel, token-isolated).** Dispatch one `.claude/agents/research/agent.md`
   sub-agent per sub-question, ALL in one message (parallel). Each runs its own Exa queries,
   reads 3–5 sources, and returns a DISTILLED report — the raw pages stay in its context, not
   yours. (`--quick` = 1 sub-agent; `--deep` = up to 5.)
3. **Merge + dedupe.** Combine the distilled reports, reconcile contradictions, synthesize a
   single coherent answer. Use Context7 directly for any version-correct library detail the
   sub-agents flagged as uncertain.
4. **Citation pass (separate).** Assemble 15–30 sources across the sub-reports into a
   references section — a deliberate pass, not inline guesses.
5. **Save.** Write the report to `.claude/research/{YYYY-MM-DD}-{slug}.md` and summarize the
   key findings + the open questions back to the user.

If no arguments were provided, ask the user what they want to research first. The
`playwright-cli` skill is for E2E test authoring, NOT research — do not use it here.
