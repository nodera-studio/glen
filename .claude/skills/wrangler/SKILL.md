---
name: wrangler
description: >-
  Drive Cloudflare across two surfaces: the `wrangler` CLI (4.101.0) for anything that
  deploys or writes — workers, R2 objects, D1 — and the Cloudflare connector/MCP for read-only
  account-level queries. Use whenever the user wants to deploy or publish a worker, put or get an
  object in R2 (including with an Object-Lock header), list R2 buckets, run a D1 query, or check
  Cloudflare account state. Trigger on indirect phrasing too — "ship the worker", "upload this
  file to the bucket", "what buckets do we have", "run this against the D1 database", "is the
  worker live". Headline rule: writes/deploys + anything headless go through the CLI; account
  inspection is read-only via the MCP. Safety-critical: if any R2 bucket uses Object-Lock, R2
  object/bucket deletes and D1 deletes should be prompt-gated — never bulk-delete a locked object.
---

# Wrangler / Cloudflare — CLI for writes, MCP for account queries

Two surfaces. Deploys and object writes are the CLI; "what do we have" is the MCP.

## CLI vs connector — when to use which

- **`wrangler` CLI (4.101.0)** — deploy/publish and object writes, headless-capable:
  `wrangler deploy` (publish a worker), `wrangler r2 object put` (with Object-Lock headers if your
  bucket uses them), `wrangler r2 object get`, `wrangler d1 execute`. The CLI is the only
  headless/CI path.
- **Cloudflare MCP/connector** — read-only account inspection: `r2_buckets_list`,
  `d1_databases_list`, `workers_list`, `search_cloudflare_documentation`. Use for orientation,
  not change.

## Project-stack facts (fill in for your project, don't re-derive each session)

- If prod R2 uses Object-Lock (_Indefinite_ or a fixed-duration mode), record which bucket(s) and
  prefix(es) are locked here, and whether the lock mode is reversible (dual-key procedure) or a
  one-way trapdoor.
- Link your own deployment runbook and architectural-decision doc for object storage here.

## Guardrails

- **Deletes are mutators → prompt-gated**, never auto-run: `wrangler r2 object delete`,
  `r2_bucket_delete`, `wrangler d1 ... delete`. **Never bulk-delete a locked object** — an
  Object-Lock posture is a compliance guarantee, not an obstacle to route around.
- Headless/cron Cloudflare work MUST be the CLI (the connector is interactive).
- Secrets: `.env` Read/Edit is permitted; never echo a token value (see the memory/secrets skill
  for the managed secret store, if configured).
