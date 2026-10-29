# Learned Rules Loop (Mnemosyne)

A "learned code-review conventions" loop: raw review/finding evidence accumulates in
your project's own Postgres, gets clustered into candidate rules, and — only via a
deliberate human paste — gets promoted into either the always-loaded `CLAUDE.md`
canon or the memory MCP's contextual store. It is separate from the Mnemosyne MCP
server project itself: this loop reads/writes tables in **your host project's own
local Postgres**, not the memory MCP's database.

Requires a `rule_findings` / `rule_candidates` table pair in your project's own
Postgres. Nothing in `scripts/mnemosyne/` creates these tables — provision them
yourself (a plain migration) using the schema below, inferred from the actual
queries the scripts run.

## Schema

### `rule_findings` — append-only evidence ledger

One row per observed finding (a review comment, a lint escape, a Sentry-escaped
defect, etc.). Never updated after insert except by the retention prune.

| Column         | Type        | Notes                                                                 |
| -------------- | ----------- | ---------------------------------------------------------------------- |
| `id`           | uuid (PK)   | generated default                                                     |
| `source`       | text        | required — where the finding came from (e.g. `code_review`, `sentry_escape`) |
| `engine`       | text        | nullable — which engine produced it (e.g. `claude`, `codex`, `sentry`) |
| `finding_text` | text        | required — the human-readable finding                                 |
| `category`     | text        | nullable — coarse class used for clustering (e.g. `rls-tenant`, `error-handling`) |
| `file`         | text        | nullable — file path the finding concerns                             |
| `accepted`     | boolean     | nullable — whether the finding was accepted/fixed vs disputed          |
| `action`       | text        | nullable — what happened (e.g. `fixed`, `escaped`, `dismissed`)        |
| `commit_sha`   | text        | nullable — commit that introduced or fixed the issue                  |
| `session_id`   | text        | nullable — groups findings from the same review/session episode       |
| `created_at`   | timestamptz | default `now()`                                                        |

Dedupe key (enforced by `record-finding.mjs` as a guard `SELECT`, not a DB
constraint): `(session_id, finding_text, file)` using `IS NOT DISTINCT FROM` so NULLs
compare as equal.

### `rule_candidates` — distilled, scored, promotable rules

One row per distinct candidate rule. Mutated by the distiller (scoring/decay) and by
the human-gated `promote.mjs` / `retire.mjs`.

| Column                     | Type        | Notes                                                                 |
| -------------------------- | ----------- | ---------------------------------------------------------------------- |
| `id`                       | uuid (PK)   | generated default                                                     |
| `rule_text`                | text        | required — one imperative sentence + why + how-to-apply                |
| `category`                 | text        | required — matches `rule_findings.category` for clustering             |
| `supporting_evidence_ids`  | uuid[]      | `rule_findings.id`s backing this candidate; unioned on merge           |
| `score`                    | int         | count of distinct independent episodes (session_id/commit_sha) backing it |
| `status`                   | text        | `candidate` \| `promoted` \| `retired`                                 |
| `tier`                     | text        | nullable until promoted — `claude_md` \| `memory_mcp`                  |
| `promoted_ref`             | text        | nullable — anchor slug (claude_md) or memory-entity id (memory_mcp)    |
| `reviewed_by`               | text        | nullable — who promoted it                                             |
| `reviewed_at`               | timestamptz | nullable — when promoted                                               |
| `last_fired_at`             | timestamptz | nullable — last time a matching finding reinforced the score           |
| `updated_at`                | timestamptz | default `now()`, bumped on every mutation                              |
| `created_at`                | timestamptz | default `now()`                                                        |

Merge-dedupe key (`record-candidate.mjs`): normalized `rule_text` (lowercased,
whitespace-collapsed) + `category`, restricted to `status = 'candidate'` rows — never
silently reopens a retired or promoted rule.

## Workflow

1. **Record.** `record-finding.mjs` appends raw findings as they occur (a review
   pass, a Sentry-escape sweep, a manual note) — one row per finding, deduped.
2. **Query.** `query-findings.mjs` dumps a recent window (default 7 days) of
   findings grouped by category, for the distiller to cluster.
3. **Distill (nightly, `distill.sh` + `distill-prompt.md`).** A headless `claude -p`
   turn clusters findings into candidates (≥3 findings from distinct episodes),
   self-checks generalizability (category-level, not file-specific; not already
   canon), checks for conflicts with live candidates, writes surviving clusters via
   `record-candidate.mjs`, then runs a decay/cap/retention sweep (fire+accept score
   bumps, time-decay, auto-retire at `score <= 0`, prune old unreferenced findings).
   **Nothing here promotes a rule** — promotion is a separate, human-gated step.
4. **Digest (weekly, `weekly-digest.sh`).** Renders a plain Markdown table of
   candidates eligible for promotion (`status='candidate' AND score>=3`), each with a
   ready-to-paste `promote.mjs` command and a tier hint. See `digest-prompt.md` for
   the tier decision rule and an optional LLM tier-recommendation variant.
5. **Promote (human-gated, `promote.mjs`).** "One click" = one paste of the digest's
   command. Flips `status -> 'promoted'`, lands the rule in its tier:
   - `claude_md` — appends a one-liner under a bounded, hard-capped section of
     `CLAUDE.md` (e.g. 15 lines) for truly cross-cutting, always-loaded conventions.
   - `memory_mcp` — records the rule in the memory MCP as a contextual/procedural
     entry, surfaced by search only when the relevant subsystem is in play.
   Evicts the lowest-scoring promoted rule if the active set would exceed the cap
   (default 30).
6. **Retire (`retire.mjs`).** Manual/explicit retirement path — flips
   `status -> 'retired'` and removes the rule from its tier. Auto-retirement of
   fully-decayed rules is handled by the nightly distiller sweep; this script is for
   an explicit human call.

## Wiring it up

- Set `DATABASE_URL` in the environment before running any of these scripts — they
  read it directly and fail loudly if unset (no hardcoded default connection
  string). Point it at your project's own Postgres, not the memory MCP's database.
- Provision the two tables above via a normal migration in your project.
- Wire `distill.sh` and `weekly-digest.sh` into cron per `scripts/cron/README.md`, or
  run them by hand while validating the loop.
- `promote.mjs` and `retire.mjs` assume they're run from inside a git repo with a
  `.claude/CLAUDE.md` at the root (for the `claude_md` tier) — adjust the path
  resolution if your `CLAUDE.md` lives elsewhere.
