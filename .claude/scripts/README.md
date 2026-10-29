# Scripts

Standalone tooling used by hooks, commands, and cron — not agent prompts, not slash
commands, just scripts something else invokes (or you run by hand).

| Path | What it does | Used by |
| --- | --- | --- |
| `bootstrap-tools.sh` | Installs a CLI productivity layer (shellcheck, gitleaks, delta, difft, ast-grep, sd, yq, scc, gron, jless, git-absorb, hyperfine, watchexec, dust, procs, hexyl, lazygit, mlr, ctags) on macOS (brew) or Linux (apt + GitHub-release binaries) | Run by hand once, when bringing up a new machine — see `docs/Claude-Code-Setup.md` |
| `statusline.sh` | Renders the Claude Code status line (model, context %, rate limits, lines changed, cost, duration) | Wired via `settings.json`'s `statusLine.command` |
| `apply-theme.sh` | Installs a custom Claude Code color theme (`themes/linear-dark.json` by default) into `~/.claude/themes/` and points `~/.claude/settings.json`'s `theme` key at it | Run by hand once per machine — see `docs/customizations/theme-setup.md` |
| `orchestrator/record-gap.mjs` | Appends a gap record to `.claude/orchestrator/gaps.jsonl` + `GapLog.md` when `/orchestrate` mode spawns a one-off subagent with no matching harness agent | Called from `agents/orchestrator/orchestrator-mode.md`, read back by `/audit-agent-gaps` |
| `brainstorm/` | A tiny local Node server (`server.cjs` + `helper.js` + `frame-template.html`) that renders browser-based mockups for the `/brainstorm` skill's optional visual-exploration mode | Started/stopped by `start-server.sh`/`stop-server.sh`, invoked from `agents/brainstorm.md` |
| `mnemosyne/` | The learned-rules-loop scripts (`record-finding.mjs`, `distill.sh`, `weekly-digest.sh`, `promote.mjs`, `retire.mjs`) — read/write your project's OWN Postgres (`DATABASE_URL`), not any Mnemosyne server's database | The `mnemosyne-capture.sh` hook (recording) + your own cron (distilling/promoting) — see `docs/LearnedRulesLoop.md` for the schema and workflow |
| `cron/` | Reference cron wiring for the Mnemosyne codebase-MCP reindex job | Nothing auto-installs this — see `cron/README.md` for the manual install steps |

## Two different "Mnemosyne" things, easy to conflate

- **`scripts/mnemosyne/`** — the learned-rules distillation loop. Reads/writes tables
  (`rule_findings`, `rule_candidates`) in **your own project's Postgres**. This is a
  harness feature: it captures episodic fixes during sessions and periodically distills
  them into promotable `CLAUDE.md` conventions. Fully optional; needs no external server.
- **`scripts/cron/mnemo-reindex.sh`** — talks to the separate **Mnemosyne MCP server**
  (the memory/codebase/secrets stack — see `skills/mnemosyne/SKILL.md`), which is its
  own project you run yourself, not something this repo ships. This script just
  triggers that server's codebase reindex on a schedule.

Neither requires the other. You can run the learned-rules loop with zero Mnemosyne MCP
server, and you can run the Mnemosyne MCP server without ever touching the learned-rules
loop.
