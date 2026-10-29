# Mnemosyne Digest — tier-decision guidance

`weekly-digest.sh` renders the digest table with a **plain node** pass (no LLM needed
to format a table). This file documents the **tier decision** the operator applies
when pasting a `promote.mjs` command, and is the prompt for the OPTIONAL `claude -p`
variant if a richer LLM tier recommendation is ever wanted.

## The two tiers

- **`claude_md`** — durable, generalizable conventions that apply **regardless of
  which file you're in**. Lands as a one-liner under the bounded
  `## Learned review conventions (Mnemosyne)` section of `.claude/CLAUDE.md` (hard
  ~15-line cap). Use ONLY for the truly cross-cutting few — every line here is
  always-loaded canon. Example: "Every new enum-like registry entry must also update
  its downstream union type and any i18n labels in the same change."

- **`memory_mcp`** — contextual / subsystem-specific patterns, gated on touching a
  specific area. Lands as a procedural `memory_store` entry surfaced by
  `memory_search` when that subsystem is in context. This is the **default** — cheaper
  to carry, doesn't bloat the always-loaded canon. Example: "When touching the queue
  retry path, remember application-level retry (a counter field + new job), not the
  queue library's built-in attempt counter."

## Decision rule

- Applies on **every** task, file-independent → `claude_md`.
- Gated on a **specific subsystem** → `memory_mcp`.
- **When unsure → `memory_mcp`** (cheaper to carry; never bloat the canon).

## Optional LLM tier-recommendation pass

If invoked via `claude -p` with `Read` access, for each eligible candidate read its
`rule_text` + `category`, apply the decision rule above, and recommend a tier with a
one-line rationale. Do NOT promote — only recommend. The operator still pastes the
`promote.mjs` command (the supervised gate).
