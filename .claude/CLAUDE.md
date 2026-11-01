# glen — Claude Code harness configuration

How the agent-os harness in `.claude/` maps onto glen. What glen is, the omp facts, the cargo
commands and the coding conventions are in the root [`AGENTS.md`](../AGENTS.md), which Claude
Code loads through the root `CLAUDE.md` and Codex reads directly. This file does not repeat them.

## Session start

Check the available skills before starting work, and recall Mnemosyne project `glen` before
substantial work (see Memory below).

## Project profile (harness tokens)

The bootstrap filled these values into the agent prompts, commands and skills under
`.claude/`. This table is the reference when the stack changes. Token names that remain in
`.claude/` are template meta only (the bootstrap agent and command, `agents/README.md`,
`docs/Claude-Code-Setup.md`, `prompts/`, `rules/*.template`).

| Token | glen |
| --- | --- |
| `{{PROJECT_NAME}}` | glen |
| `{{PRIMARY_LANGUAGE}}` | Rust, stable toolchain, edition 2024. The TypeScript omp extension runs under omp’s Bun runtime and is tested under Node. |
| `{{FRAMEWORK}}` | clap 4 (derive) + clap_complete; serde/serde_json; thiserror (the `GlenError` type, used in both the binary and the library, no `anyhow`); anstyle/anstream colour, comfy-table tables, indicatif spinners, terminal_size; dirs; omp-rpc (git tag v18.14.0), uuid (v7 job ids), libc (Unix, process start time); rustix (lease probe and process feature, Unix), xxhash-rust (Linux lease name), jiff (timestamps), unicode-width, signal-hook (tail -f SIGINT, Unix), ratatui 0.30 (crossterm 0.29 through `ratatui::crossterm`), insta (dev). Static Astro 7 with React 19 islands for `site/`. |
| `{{PACKAGE_MANAGER}}` | cargo; `Cargo.lock` is committed and CI builds with `--locked`; npm for `site/` (`package-lock.json` committed, CI `npm ci`) |
| `{{MONOREPO}}` | no Cargo workspace; `site/` is a separate npm project |
| `{{FRONTEND_DIR}}` | `site/`: static Astro 7 site with React 19 islands and Tailwind v4; Node from `site/.nvmrc` |
| `{{BACKEND_DIR}}` | `src/`: library `src/lib.rs` plus a thin binary `src/main.rs` (there is no backend service) |
| `{{SHARED_DIR}}` | none |
| `{{DEV_CMD}}` | `cargo run -- <args>`; `npm --prefix site run dev` |
| `{{BUILD_CMD}}` | `cargo build` (`cargo build --release` for an optimised binary); `npm --prefix site run build` |
| `{{TEST_CMD}}` | `cargo test` (CI form: `cargo test --locked`; tests/extension_ts.rs needs Node ≥22.6); `npm --prefix site run test:e2e` / `npm --prefix site run test:unit` |
| `{{TYPECHECK_CMD}}` | `cargo check --all-targets`; `npm --prefix site run check` |
| `{{LINT_CMD}}` | `cargo clippy --all-targets -- -D warnings` |
| `{{FORMAT_CMD}}` | `cargo fmt --all` writes; `cargo fmt --all -- --check` is the report-only form for CI-style runs |
| `{{VCS_DEFAULT_BRANCH}}` | `main` |
| `{{DOCKER}}` | none: glen does not use Docker |
| `{{QUEUE_OR_TOPIC_1}}`, `{{QUEUE_OR_TOPIC_2}}` | none: no queues or background jobs |
| `{{ISSUE_KEY_PREFIX}}` | none: the Mnemosyne workspace API exposes no keys for issues (`document_create`, `document_get` and `board_list` return only a UUID `id`). Refer to an issue by its wave label plus document id, e.g. `W0 (6e0f7c78-8d28-4a0b-9b4a-f2569545293e)`. Branch names and PR titles carry the wave label (`feat/v010-wave-0-…`, `W0: …`). |

### Reading web-shaped agent prompts against glen

Several agent prompts were written for a TypeScript web monorepo. Apply them to glen like this:

- glen's only frontend is the static site in `site/`; it has no backend service, database, ORM,
  queue, Docker or error tracker, and its hosting is an assets-only Cloudflare Worker
  (`site/wrangler.jsonc`, no bindings). Mark those areas n/a.
- For `site/`, npm, tsc and Playwright mean the site's own scripts.
- For Rust tooling: npm/pnpm means cargo; tsc or "typecheck" means `cargo check --all-targets`; eslint means
  clippy; prettier means `cargo fmt`; jest/vitest filters mean `cargo test <filter>`; `npm audit`
  and `npm outdated` mean `cargo audit` and `cargo outdated` (neither is installed yet).
- "Logs" are the CLI's own stderr and omp's files under `~/.omp`. "Runtime context" is a local
  reproduction with `cargo run -- <args>` against the installed omp (18.14.0), not a monitoring
  service.
- Tests live in `#[cfg(test)]` modules and under `tests/`; omp data comes from `tests/fixtures/`.
  The `/implement` conformance writer puts its suite in `tests/conformance.rs` (submodules under
  `tests/conformance/`) and runs it with `cargo test --test conformance`.
- There is no dev stack to start: skip "ensure Docker / the dev stack is running". Manual testing
  is `cargo run -- <args>`.
- Review engines are Claude, Codex and omp (third engine: a Cursor model, read-only, via glen and
  `.claude/scripts/omp-engine.sh`; skipped with a note when `omp-engine.sh probe` fails).
- The `/implement` re-index step (Role 4) applies only if `code_index_status` with project `glen`
  shows an index; otherwise skip it and say so.
- The Implementer's deterministic gate is `.claude/scripts/quality-gate.sh` (rustfmt check +
  clippy `-D warnings`, also for the Linux target on macOS, plus the site checks when `site/` or `src/render/palette.json` changed); `/implement-quick`'s fail-safe classifier is
  `.claude/scripts/quick-classify.sh`. Where a prompt says `scripts/quality-gate.sh`, use these.
- `/run-ci`, `/docs-audit`, `/tech-debt-audit`, `/error-audit` and the `/codebase-audit` scans
  (including the dependency audit) are ported to cargo and `rg`. `cargo-audit`, `cargo-outdated`,
  `cargo-deny` and `cargo-license` are not installed: the audits probe for them, fall back, and
  report the gap.

## Workflows

The full agent index is `.claude/agents/README.md`; commands are listed in
`.claude/commands/README.md`.

| Command | Use for |
| --- | --- |
| `/plan-implementation` → `/implement` (`/implement-quick` for small changes) | Plan, then build with the routed Implementer (`routing.sh implementation`, Claude by default), the Claude ‖ Codex ‖ omp review gate and the conformance tail |
| `/code-review`, `/respond-to-review` | Review the current diff (Claude ‖ Codex, + omp when ready); answer PR review comments |
| `/review-implementation` | Claude ‖ Codex ‖ omp |
| `/deep-debug` | Claude ‖ Codex investigator (+ optional omp), then the adversary |
| `/run-ci` | Run the same checks as `.github/workflows/ci.yml` locally |
| `/goal` + `/auto-mode` | Carry a multi-wave goal to merged completion on `main` |
| `/explore`, `/research`, `/brainstorm` | Map code, research a topic with Exa, turn an idea into requirements |
| `/security-audit`, `/error-audit`, `/docs-audit`, `/tech-debt-audit`, `/codebase-audit` | Periodic whole-repo audits |
| `/orchestrate`, `/audit-agent-gaps`, `/codex`, `/codex-advisor`, `/improve-prompt`, `/security-scan` | Harness and cross-model utilities |

Removed for glen, so do not look for them: `/copywrite` and its agents (no localised UI copy),
`/api-audit` (written for a NestJS HTTP API), the frontend error-audit arm, and the `linear`,
`playwright-cli`, `sentry-cli`, `wrangler` and `promote-to-prod` skills. `promote-to-prod` became
the `release` skill.

## Git hooks

`lefthook.yml` at the repo root, described in `AGENTS.md`: pre-commit checks rustfmt and runs
gitleaks on staged changes; pre-push runs clippy and the tests. Run `lefthook install` once per
clone (`brew install lefthook` first if it is missing). Bypass with `--no-verify` only with a
written reason.

## Branching and releases

- Trunk is `main`. There is no staging branch and no release branch.
- Branch off `main` (`feat/<slug>`, `fix/<slug>`, `chore/<slug>`), open the PR into `main`, and
  squash-merge it. Never push to `main` directly.
- Before merging a PR, store its durable decisions in Mnemosyne; the `capture-before-merge` hook
  blocks `gh pr merge` until a `memory_store` call referencing the PR exists in the session.
- A release is an annotated tag `v0.x.y` on a commit on `main`, after a version-bump PR. The
  runbook is the `release` skill. Day-to-day rules are in the `branching-model` skill.

## Issue tracking

Issues live on the Mnemosyne board of project **Glen** (project name `glen`, case-insensitive;
workspace `projectId` `ff92abe1-da63-402d-a4f1-c8690e471be1`, taken from the 2026-10-30 handoff).
Not Linear, not GitHub Issues. Issues have no keys: refer to one as `W<n>` plus its document id; board calls take the full id. Tags `shipped:main` and `released:v0.x.y` mark
where an issue's code is. Board operations are in
the `mnemosyne` skill. Don't track work that belongs on the board in a TODO file.

## Memory, code index and secrets

Mnemosyne through the `mnemosyne` plugin. Tools are named `mcp__plugin_mnemosyne_<server>__<tool>`
(servers `memory`, `codebase`, `secrets`, `workspace`). Always pass `project: "glen"` on memory,
code and secrets calls, and the `projectId` above on workspace calls. omp-as-a-donor research
belongs to project `metis`, harness questions to `agent-os`, machine and toolchain setup to
`ops`. The `mnemosyne` skill in `.claude/skills/` is the glen-specific guide; the plugin's own
skills (`mnemosyne:mnemosyne`, `mnemosyne:mnemosyne-workspace`) are the full manual.

## Codex

Roles come from `.claude/routing.json`, read through `bash .claude/scripts/routing.sh <key>`
(an owner-confirmed override in `.claude/auto-mode/routing.json` wins). Current (decision
148655d9): Claude implements (`impl-coder-claude`); Codex reviews, plans, debugs and audits.
Dispatch Codex only through the `codex:codex-rescue` agent, `codex-companion.mjs` (the
Implementer, only when `routing.sh implementation` prints `codex`) or `codex review`; the
`block-old-codex` hook blocks `codex exec`. Harness prompts do not pass a Codex model; model
choice is configured outside the harness (see `AGENTS.md`).

## omp

The third review/debug engine uses profile `glen-harness` on Cursor.
Sessions are read-only under omp's approval policy plus read containment (`.claude/omp/glen-harness.yml`).
Configure it with `.claude/scripts/omp-profile-setup.sh`; see `.claude/skills/omp/SKILL.md`.
Claude Code's `permissions.deny` rules are best-effort typing guardrails, not a security boundary.
Default-profile protection relies on explicit `--profile glen-harness` in harness scripts,
setup's fingerprint check, and the OS sandbox.
Set `GLEN_OMP_ENGINE=off` to skip the engine.

## Tool routing

The CLI tool table (rg, sd, ast-grep, difft, yq, gron, scc) is in
`.claude/docs/Claude-Code-Setup.md`; `hooks/steer-bash.sh` enforces part of it.

| Need | Route to | Boundary |
| --- | --- | --- |
| Literal string or symbol in this repo | `rg` | not the codebase MCP |
| How omp works | `../metis/references/oh-my-pi` (read-only, v18.14.0) and `.claude/research/` | never edit or push the clone |
| Library or crate docs | `ctx7` CLI (Context7), `/find-docs` | not web search |
| Web pages, articles, current state of a tool | Exa MCP, `/exa` | native WebSearch and WebFetch are denied |
| Decisions and facts across sessions | Mnemosyne memory, project `glen` | not ad-hoc notes |
| Issues | Mnemosyne board, `mnemosyne` skill | not Linear, not GitHub Issues |
| Secrets | Mnemosyne secrets, project `glen` | never echo a value |
| PRs, CI checks, run logs | `gh`, `/gh` | git stays git |
| Second opinion, hard bug | `codex:codex-rescue`, `/codex` | never `codex exec` |
| Third-model review or debug pass | `.claude/skills/omp/SKILL.md` → `.claude/scripts/omp-engine.sh` | never `omp` directly, never the default profile |

## `.claude/` layout

| Directory | Purpose |
| --- | --- |
| `agents/`, `commands/`, `skills/` | Agent prompts, slash commands, skills |
| `hooks/` | Enforcement scripts wired in `settings.json` |
| `rules/` | Path-scoped conventions; `behavioral-preamble.md` is the one always-loaded rule |
| `routing.json` | Who does each kind of work (implementation, UI, review, planning, debug, audits) and the fan-out caps; read it through `scripts/routing.sh`, never directly |
| `docs/`, `prompts/`, `scripts/` | Harness reference, prompt-engineering guides, tooling |
| `plans/`, `research/`, `handoffs/`, `evidence/`, `tech-debt/`, `testing/` | Working material, git-ignored; plans are kept after execution as history |
| `auto-mode/` | `/goal` and `/auto-mode` state file (git-ignored: per-machine loop state) |
| `reviews/` | Audit reports |

## Response style

- Be brief: prefer fewer complete sentences over more.
- Don't reproduce code, diffs or file contents you just wrote; reference `path:line`.
- No preamble or sign-offs. State the result once.

## Learned review conventions

Not adopted. The learned-rules loop (`.claude/scripts/mnemosyne/`, `docs/LearnedRulesLoop.md`)
needs a project Postgres, which glen does not have, so its Stop hook is not wired.
