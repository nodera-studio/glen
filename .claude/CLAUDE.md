# {{PROJECT_NAME}} — Claude Code Configuration

**This is the primary Claude Code configuration for the {{PROJECT_NAME}} repo.**

## Project at a glance

**{{PROJECT_NAME}}** — {{PROJECT_DESCRIPTION}}

Stack:

- **Frontend**: {{FRONTEND_STACK}}
- **Backend**: {{BACKEND_STACK}}
- **DB**: {{DATABASE_STACK}}
- **Hosting**: {{HOSTING_STACK}}

## Session Start

At the start of every conversation, check available skills (via the skills list in the system prompt) before doing anything else. Use skills when they match the task at hand.

## Commands

```bash
{{DEV_CMD}}         # Start local dev environment
{{BUILD_CMD}}       # Production build
{{TEST_CMD}}        # Run tests
{{TYPECHECK_CMD}}   # Type-check
{{LINT_CMD}}        # Lint
{{FORMAT_CMD}}      # Format
```

## Git hooks

If this project uses `lefthook` (or an equivalent pre-commit/pre-push runner), document
what it enforces here (lint/format on staged files, secret scanning, the test suites
that gate `git push`) and how to bypass it deliberately (`--no-verify`) when justified.

## Branching & release model

Document your branching model here — trunk name, release-branch name (if any), PR
target, squash vs merge-commit policy, and any branch-protection rulesets. If you use
the `/branching-model` skill pattern from this harness, describe day-to-day branch/PR
rules there and keep this section to the summary.

## `.claude/` Directory Structure

| Directory    | Purpose                                                                                                                                                         | Access                                                     |
| ------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------- |
| `agents/`    | Orchestration prompts for complex multi-step tasks                                                                                                              | Read freely                                                |
| `skills/`    | Reusable capabilities, invoked via `/skill-name`                                                                                                                | Invoke via `/skill-name`                                   |
| `commands/`  | Slash commands that kick off agent workflows                                                                                                                    | Invoke via `/command-name`                                 |
| `hooks/`     | Enforcement scripts (PreToolUse/PostToolUse/etc.)                                                                                                               | Automatic                                                  |
| `rules/`     | Path-scoped conventions, loaded automatically when editing matching files                                                                                       | Automatic                                                  |
| `docs/`      | Harness reference docs (setup, security posture, statusline, etc.)                                                                                              | Read freely                                                |
| `scripts/`   | Standalone tooling invoked by hooks, commands, or cron (bootstrap, statusline, learned-rules loop)                                                               | Read/run on demand                                         |
| `prompts/`   | General-purpose prompt-engineering reference material                                                                                                           | Read on demand                                             |
| `plans/`     | Implementation plans — kept after execution (marked completed, NOT deleted) so the directory accumulates as project history                                     | Read when planning or reviewing past decisions             |
| `research/`  | Research reports — topic deep-dives, library comparisons                                                                                                        | Read on demand when relevant                               |
| `tech-debt/` | Tech debt registry — categorized items from reviews, audits, and manual findings                                                                                | Read when planning work or fixing debt                     |
| `testing/`   | Test plan TODO (supervised test writing)                                                                                                                        | Read when working on tests                                 |
| `evidence/`  | Captured evidence artifacts from audits/reviews (screenshots, logs, reports)                                                                                    | Read on demand                                             |
| `handoffs/`  | Session handoff notes between agents or sessions                                                                                                                | Read on demand                                             |
| `orchestrator/` | `/orchestrate` mode's gap log (`GapLog.md` + `gaps.jsonl`) — one-off subagent spawns with no matching harness agent, clustered periodically by `/audit-agent-gaps` | Read when reviewing gaps or running `/audit-agent-gaps`  |

### Agents

Full agent suite documented in `.claude/agents/README.md`. Fill in your project's key
workflows here once `/bootstrap-project` (or manual setup) has wired the agent suite —
e.g. a table mapping `/plan-implementation`, `/implement`, `/review-implementation`,
`/debug`, and any audit commands to the agents they dispatch.

`/orchestrate` (dispatch-only mode) and `/audit-agent-gaps` (cluster the gap log, propose
promoting recurring one-off shapes into permanent agents) are pre-wired and generic — no
per-project filling-in needed, see `agents/orchestrator/README.md`.

### Skills & Tools

Use these proactively — not just inside agent workflows. If a task matches the trigger, invoke the skill via the Skill tool.

| Skill      | Trigger — use when...                                          |
| ---------- | ---------------------------------------------------------------- |
| `/find-docs` (Context7) | Working with ANY library API — verify patterns against current docs, not training data. |
| `/research` | Deep-diving any topic — library comparison, best practices, architecture decisions. |
| _(add project-specific skills here as they're installed)_ | |

## Reference Documents (read on demand, not at startup)

Point at your own harness setup doc, e.g. `.claude/docs/Claude-Code-Setup.md`, and any
other on-demand reference docs you add.

## Response style

Adjust to taste — this is a common starting point:

- Be brief: prefer fewer complete sentences over more.
- Don't reproduce code, diffs, or file contents you just wrote; reference `path:line`.
- No preamble or sign-offs. State the result once; don't re-explain your work.

## Tool routing

If this project wires up MCP servers, CLIs, or other tools with overlapping
responsibilities, keep a terse routing table here so the judgment call ("which tool for
this need") is always-loaded instead of re-derived every session. Start from the CLI
tool-routing table in `docs/Claude-Code-Setup.md` (rg vs grep, sd vs sed, ast-grep,
difft, yq, gron, scc, the Exa-vs-curl boundary — the tools `bootstrap-tools.sh`
installs and `hooks/steer-bash.sh` partly enforces) and append this project's own
MCP/service routing rows below it. Pattern:

| Need                                   | Route to        | Why / boundary                                          | → skill      |
| --------------------------------------- | ---------------- | -------------------------------------------------------- | ------------ |
| [e.g. "literal in-loop string match"]   | `rg` / grep      | fast, exact — not routed through a heavier tool           | —            |
| [e.g. "web search"]                     | [your web-search MCP/tool] | [why this over a native tool, if one is denied] | `/[skill]`   |
| [e.g. "remember a decision across sessions"] | [your memory MCP/tool] | context that must survive compaction         | `/[skill]`   |

## Learned review conventions

> If you adopt the learned-rules loop (`.claude/scripts/mnemosyne/` +
> `docs/LearnedRulesLoop.md`), this section is auto-maintained by
> `.claude/scripts/mnemosyne/promote.mjs` — do not hand-edit. Keep it hard-capped
> (e.g. 15 lines); lowest-value rules decay out over time. Promotion should be
> human-gated (a deliberately pasted `promote.mjs` command), not automatic.
