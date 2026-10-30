# glen — Agent Guide

Shared guide for every coding agent working in this repo (Codex reads this file; Claude Code
loads it through `CLAUDE.md`). Claude Code's harness configuration lives in `.claude/CLAUDE.md`.

## What glen is

A Rust CLI that lets Claude Code and Codex **see and talk to omp (oh-my-pi) sessions**, and lets
omp talk back:

- **discover** omp sessions, live and past, across projects;
- **read** a session's transcript, as text or structured events;
- **send** a message into a session (prompt, steer, follow-up) and **receive** the reply;
- give the omp model a way to **reply to the calling agent**.

Product direction (owner, 2026-10-30; details in the Glen project's Ideas in Mnemosyne):

- **Standalone and agent-callable, the `playwright-cli` way.** One binary with short verb-first
  subcommands, short session handles, human output on a TTY and `--json` for agents, and a
  shipped agent skill (`SKILL.md`) for Claude Code and Codex. No MCP server needed to use it.
- **It has to look good.** Polished terminal output and a live full-screen session view.
- **An Astro landing page** (`site/`) in the Nodex redesign's look (`apps/mockup-nodex`;
  Mnemosyne decision `f6a568ef`), with morphin.dev motion components.

Design and the omp surfaces it builds on: [`docs/design.md`](docs/design.md).

## omp facts that matter

- Target version: **omp 18.14.0** (`omp --version`). The source is at
  `../metis/references/oh-my-pi` (a reference clone, MIT; read it, never edit or push it).
  When omp updates, re-check the surfaces in `docs/design.md` against the new source before
  relying on them: omp ships several releases a week and its RPC/session code moves fast.
- omp home: `~/.omp` (`PI_CONFIG_DIR`), agent dir `~/.omp/agent` (`PI_CODING_AGENT_DIR`,
  profiles under `~/.omp/profiles/<name>/agent`).
- Never write to an omp session's `.jsonl` file. omp owns it; glen reads it, and sends through
  omp (RPC or the glen bridge extension) so omp does the writing.

## Commands

```bash
cargo build                                         # debug build
cargo run -- doctor                                 # check omp, paths and glen home
cargo run -- <args>                                 # run the CLI
cargo test                                          # all tests
cargo clippy --all-targets -- -D warnings           # lint (warnings fail CI)
cargo fmt --all                                     # format
```

Site:

```bash
cd site && npm ci   # Node from site/.nvmrc
npm run dev
npm run verify      # astro check, build, timeline tests, budgets
npm run test:e2e    # Playwright smoke; npx playwright install chromium once
npm run lhci        # Lighthouse CI, mobile, light and dark
```

`lefthook install` once per clone: pre-commit checks rustfmt and scans for secrets, pre-push runs
clippy and the tests. CI (`.github/workflows/ci.yml`) runs the same on Linux and macOS (the
`site` job runs on Linux only).

## Conventions

- Rust stable, edition 2024. Errors: `GlenError` (`thiserror`) throughout, binary and library; no `anyhow`.
- Parse omp's formats leniently: unknown entry types and fields must be skipped, not fatal.
  omp adds entry types between releases.
- Tests that need omp data use fixtures under `tests/fixtures/`, never a live `~/.omp`.
- Tests that need an RPC peer use the `fake_omp` example (`tests/support/fake_omp.rs`) through `GLEN_OMP_BIN`, never a live omp.
- Never mutate the process environment in code or tests (edition 2024 `set_var` is unsafe); read through `env::Env`.
- Branch off `main`, PR into `main`. No direct pushes to `main`.
- `site/` never imports `framer-motion`; motion comes from `motion/react` through `IslandRoot`.
  morphin items are refreshed in `site/vendor/morphin/` and ported by hand (`site/morphin.json`).
- The CLI palette lives in `src/render/palette.json`; `palette_contract` keeps `style()` in step.

## Mnemosyne

Project **`glen`**. Recall before substantial work (`memory_search`, and `memory_list` with
`sourceKind: "decision"`); record decisions, confirmed omp facts, procedures and tech debt when
done. omp-as-a-donor research for Metis lives in project `metis`; harness questions go to
`agent-os`; machine and toolchain setup goes to `ops`.

## Codex

Codex is the second engine in the agent-os review, debug and audit workflows, dispatched through
the `codex:codex-rescue` agent (direct `codex exec` is blocked by a hook). Codex runs `gpt-6.1-sol`
at `high` effort and reaches Mnemosyne through the same token as Claude Code. `gpt-6.1-sol` needs
Codex CLI 0.166 or newer: older CLIs get "model is not supported when using Codex with a ChatGPT
account".
