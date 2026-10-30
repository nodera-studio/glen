# Skills — index note

A skill is a `SKILL.md` file with YAML frontmatter (`name`, `description` with a clear
trigger phrasing) plus a body documenting WHEN to use it and HOW. Use the skills here as the
structural reference when you write your own.

## Skills in glen

| Skill | Use for |
| --- | --- |
| `auto-mode` | The `/goal` + `/auto-mode` wave loop: waves PR into `main`, tracked on the Glen board |
| `branching-model` | Trunk-only branch, PR and merge rules (`main`, squash-merge, capture before merge) |
| `release` | Version-bump PR, then an annotated `v0.x.y` tag on `main`; board issues tagged `released:v0.x.y` |
| `mnemosyne` | glen's Mnemosyne ids, memory conventions and board operations |
| `codex` | Dispatching Codex through `codex:codex-rescue` / `codex review` |
| `exa` | All web access (native WebSearch/WebFetch are denied) |
| `gh` | PRs, checks, run logs, merges |

Removed for glen: `linear` (issues live on the Mnemosyne board), `playwright-cli` (no browser
UI), `sentry-cli` (no error tracker), `wrangler` (the global `cloudflare:wrangler` plugin skill
covers any Cloudflare work, e.g. the future landing page) and `promote-to-prod` (no staging
branch; the `release` skill replaced it).

## Add project-specific skills as your project needs them

Some categories of capability are common enough that most real projects eventually want
a skill for them, but specific enough to each project's actual stack that a generic
template shouldn't ship one pre-written. Write your own once the corresponding part of
your stack exists:

- **Billing integration** — your payment provider's CLI/MCP, webhook conventions, test-mode setup.
- **Database/backend platform** — your managed-database CLI, row-level-security patterns, ORM conventions.
- **Deploy target** — your hosting platform's CLI/compose conventions, how a release actually ships.
- **Backend scaffolding** — a generator for your framework + ORM + validation-library module conventions.
- **Design system** — your design-token/color-palette/component conventions.
- **Background-job debugging** — your queue library's (BullMQ, Sidekiq, Celery, SQS) retry semantics, stuck-job diagnostics, the DI footgun you don't want rediscovered.
- **CI/CD** — your pipeline's structure, which jobs gate a merge, how to re-run a flaky job.
- **Feature flags** — your flagging provider's conventions, naming rules, rollout procedure.
- **Observability** — how to pull a trace/error from your APM tool, which dashboards and alerts matter.
- **Data migration** — your project's backfill/dual-write/cutover pattern for large table changes.

Write each one the same way the shipped skills here are written: a trigger
`description` specific enough that Claude reaches for it at the right moment, and a
body that documents the exact commands/conventions — the mistake you don't want
repeated, the command that actually works for this project.

## Claude connectors — account-level, documented in a skill anyway

A **claude.ai-managed connector** is an OAuth-based integration (a database platform, an
error tracker, a product-analytics tool, an issue tracker, a payments platform, and
similar) that you enable once via the Claude web UI's connector settings. Its
configuration lives on the Anthropic account, not in this repo — there's no file for it
to appear in.

That doesn't mean it goes undocumented. Once a connector is enabled, write a skill for
it exactly like any other: a `SKILL.md` describing WHEN to reach for its tools and HOW —
e.g. "pull the error tracker's issue + trace context before hypothesizing a root cause"
is a real, useful, committable skill. The skill documents the usage pattern; the
connector's own URL/auth stays where account settings live. A **custom connector** (an
MCP server your team built and registered privately, rather than one from the
marketplace) works the same way — document the routing/usage pattern here, register
the connection itself wherever your team manages account/workspace settings.

## Skills this template DOES wire up

Two integrations get real client-side wiring in this repo, because their configuration
is a portable setup step rather than an account-level OAuth grant:

- **Mnemosyne** (memory, codebase, secrets and workspace servers, via the `mnemosyne`
  plugin) — see `skills/mnemosyne/SKILL.md`. Client wiring only; the servers themselves
  are a separate project.
- **Exa** (web search/fetch) — see `skills/exa/SKILL.md`; a standard MCP server you
  configure once, not an OAuth connector.

Follow the same pattern for any project-local MCP server you add: a `SKILL.md`
documenting when/how to use its tools, plus whatever server config your setup docs call for.
