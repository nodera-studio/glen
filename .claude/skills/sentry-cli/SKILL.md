---
name: sentry-cli
description: >-
  Work with Sentry across two surfaces: the `sentry-cli` (3.5.1) for release/deploy
  plumbing, and the Sentry MCP for runtime issue lookup. Use the CLI whenever the user wants to
  upload sourcemaps, create or finalize a release, or associate commits with a deploy. Use the
  MCP whenever the user wants to look up a live Sentry issue, run Seer on an error, find the stack
  trace for a crash, or search recent events. Trigger on indirect phrasing too — "why is
  production throwing this", "what's the error rate for X", "pull up that exception", "map this
  minified stack to source", "tag this release". Headline rule: release/deploy + headless/CI go
  through the CLI; debugging a live error is the MCP (find the project, search issues/events, run
  Seer).
---

# sentry-cli / Sentry MCP — release plumbing vs runtime debugging

## CLI vs MCP — when to use which

- **`sentry-cli` (3.5.1)** — release/deploy plumbing, headless/CI:
  `sentry-cli releases new`, `sentry-cli releases finalize`, `sentry-cli sourcemaps upload`,
  commit association. This is the deploy-time path.
- **Sentry MCP** — runtime issue lookup when debugging a live error (mirrors a debugging
  investigator agent's first step, if you have one — this is the tool surface it reaches for):
  - `find_organizations`, `find_projects` — locate your project first.
  - `search_issues`, `search_events` — find the failing issue / its events.
  - `get_sentry_resource` — pull the full issue/event detail + stack trace.
  - `analyze_issue_with_seer` — Seer root-cause analysis.
  - `update_issue` is a mutator (assign/resolve) → prompt-gated.

## WHEN to reach for it

Debugging an application error → MCP first (project → issue → event → Seer), then trace into the
code with the Read tool / LSP. Shipping a build → CLI (release + sourcemaps) so the stack traces
the MCP later shows are symbolicated.

## Guardrails

- Headless/CI Sentry work is the CLI; the MCP is in-session debugging.
- `update_issue` mutates Sentry state → prompt-gated; don't auto-resolve.
- Secrets: never echo a Sentry auth token; `.env` Read/Edit is permitted (see the memory/secrets
  skill for the managed secret store, if configured).
