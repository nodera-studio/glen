---
name: mnemosyne
description: >-
  Use your local "Mnemosyne" MCP stack — three private Postgres-backed servers with NO
  public docs, invisible unless you know they exist: the memory MCP (mcp__memory__*) for durable
  cross-session memory, the codebase MCP (mcp__codebase__*) for semantic code orientation, and
  the secrets MCP (mcp__secrets__*) for the managed secret store. Reach for this whenever the
  user wants to remember something for later or across sessions, recall "what did we decide about
  X", store or look up a project decision, search past sessions, orient with "where does X live /
  what handles Y" before diving into code, check whether the code index is fresh, or read a
  managed secret. Trigger on indirect phrasing too — "make a note so we don't forget", "did we
  already settle the approach for Y", "find where the invoice totals are computed", "this context
  needs to survive compaction". It also routes AWAY from the wrong tool: literal in-loop string
  matches are `rg`, not the codebase MCP; throwaway notes are not memory.
---

# Mnemosyne — your local MCP stack (memory · codebase · secrets)

Three private servers on the dev box, all Postgres-backed, **no public docs**. They are easy to
forget because nothing in a stock Claude Code session advertises them — this skill is their
manual.

> The server code lives in a separate sibling repo (a Mnemosyne server project you run via
> `docker compose` locally — see that repo's own README for setup). This skill only documents how
> to USE the three MCP tools once they're running; it does not set them up.

## The three servers, one stack

| Server       | Host             | Tools                                                                                                                                    | WHEN                                                  |
| ------------ | ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------- |
| **memory**   | `localhost:8080` | `memory_search`, `memory_get`, `memory_get_recent`, `memory_get_entity`, `memory_list`, `memory_store`, `memory_update`, `memory_delete` | durable facts/decisions that must outlive the session |
| **codebase** | `localhost:8081` | `code_search`, `code_get_file`, `code_index_status`, `code_reindex`                                                                      | semantic "where does X live" — orientation/planning   |
| **secrets**  | `localhost:8082` | `secret_get`, `secret_list`, `secret_set`, `secret_delete`                                                                               | the managed secret store                              |

## memory — durable cross-session memory

**WHEN:** query at session start for prior decisions on the task; store a memory mid-session when
a non-obvious decision is made or the user says "remember this"; **context that must survive
compaction goes here, not ad-hoc notes** (`progress.md` complements for in-flight task state).

**The 4-type taxonomy** (the `type` enum on `memory_store` — write consistent types so search
stays clean):

- **episodic** — what happened (a fix that was applied, a turn outcome).
- **semantic** — a durable fact or decision (the canonical type for "we decided X").
- **procedural** — how-to (a repeatable runbook).
- **entity** — a person / project / issue / artifact (e.g. a phase-tracking `tooling-artifact`
  record).

`memory_store` requires `{type, title, content}`; optional `importance` (0–1), `sourceKind`
(tech-debt / research / plan / audit / decision / implementation / requirements / copywrite),
`tags[]`, `metadata{}`. Read shapes:

- `memory_search` `{query, type?, limit?}` → top-k `{id, title, type, snippet}`.
- `memory_get` `{id}` → full text for a search hit.
- `memory_get_recent` → latest memories (session-start orientation).

**You rarely write by hand.** The Stop hook `.claude/hooks/mnemosyne-capture.sh` (opt-in
`MNEMOSYNE_CAPTURE=1`) captures episodic fixes, and the nightly distiller
(`.claude/scripts/mnemosyne/distill.sh`) promotes patterns — don't fight the auto-writer. Use
`memory_store` for explicit, durable decisions worth recalling verbatim.

## codebase — semantic orientation

**WHEN:** orientation and planning, **before** grep — `code_search` answers "where does the
state machine for X live" better than a blind `rg`. It is **not** a replacement for literal
matching: a known string or symbol → `rg`. The index **reflects the last sync** (re-indexed
after merges); treat hits as orientation and re-grep if the area looks stale. `code_index_status`
tells you how fresh it is; `code_reindex` is a mutator (prompt-gated — the pipeline re-indexes
after merges already).

## secrets — the managed secret store

**WHEN:** reading a **managed** secret. Reconcile to the current posture: reading/editing `.env`
via the Read/Edit tools is permitted under full-autonomy, and the secrets MCP is the managed
store on top of that. `secret_get` / `secret_list` are the read path. **`secret_set` and
`secret_delete` are mutators → denied** (see your permissions config's deny list). **Never echo a
secret value** into logs, PRs, commits, or context.

## Guardrails

- Memory is **shared across sessions** — never store transient state or PII; one entity per
  durable fact, not one per file touched, so the store stays legible.
- The codebase index can be **stale** — orientation, not ground truth. Verify hot paths with the
  Read tool / `rg` before acting.
- **Never bundle these MCPs into a skill or plugin** — a plugin-bundled MCP that duplicates a
  configured server is silently skipped (CC ≥ v2.1.71). They are configured at the box level;
  this skill is guidance only.
- Never `secret_set` a real value into a committed file.
