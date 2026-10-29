---
name: codex
description: >
  Dispatch a task to OpenAI Codex through the installed `codex:codex-rescue` plugin
  agent. Use when the user wants a second-opinion review, a deep-dive on a hard
  bug, an independent implementation pass, or any time they type `/codex <prompt>`.
  Also use proactively when Claude is stuck on a substantial debugging or
  implementation problem and a different model's perspective is likely to help.
  Triggers on: "ask codex", "second opinion", "have codex look at this", "fire
  codex", "/codex", being stuck on a multi-step bug, wanting parallel investigation,
  review-implementation orchestrators needing the Codex engine.
allowed-tools: Agent, Bash
argument-hint: '<task or question to dispatch to Codex>'
---

# Codex Dispatch

## PRIMARY ROLE: Product-code implementer (the fused coder)

Codex is the **sole writer of product code** in this harness. Claude orchestrates,
plans, reviews, and writes the independent conformance tests — it does **not** write
product code. Codex fills three roles here, in priority order:

1. **Implementer (primary)** — the fused write → wire → harden → test → gate → fix
   loop for `/implement` (Role 1) and `/implement-quick`, run in Codex's own context
   via the **`coder-codex.md`** agent (`codex-companion.mjs task --json --write`).
   Codex reads its conventions from the repo-root `AGENTS.md`. This is not the thin
   `codex:codex-rescue` forwarder — it owns the `touchedFiles`/completion-report
   contract the orchestrator needs.
2. **Advisor** — the **Step 4.5 advisor gate** in `/implement` and `/codex-advisor`:
   a read-only adversarial critique of the approved plan (verdict/risks/steelman) via
   the **`codex:codex-rescue`** agent — NOT `codex review`, NEVER `codex exec`.
   **Synthesize, never forward**; ONE reconcile call on disagreement; escalate
   unresolved CRITICAL risks; gate the call rate.
3. **Review engine** — one of the three engines in the Role 2 / `/review-implementation`
   review, via the native **`codex review --base <BASE>`** diff reviewer.

`codex exec` is never used — Codex is invoked only through the plugin runtime
(`coder-codex.md` → `codex-companion.mjs`, `codex:codex-rescue`, or `codex review`).

Hand a task to OpenAI Codex through the `codex:codex-rescue` plugin subagent. This
skill is a procedural guide — it tells you HOW to invoke Codex, not what Codex
should do.

## When to use

- **Second opinion / dual-engine review** — you want a different model to look at
  the same diff, design, or root-cause hypothesis.
- **Stuck on a hard bug** — Claude has tried 2+ approaches and isn't converging.
- **Substantial implementation handoff** — a multi-step coding task you'd rather
  delegate than do inline.
- **Parallel passes** — review-implementation needs N Codex passes alongside N
  Claude passes (see Multi-pass section).

## When NOT to use

- Trivial lookups, single-file edits, anything Claude can finish in <60 seconds.
- Repository inspection, file reading, status-checking — do those yourself first
  and only forward a tight, well-scoped prompt to Codex.
- Quick clarifications — just ask the user.

## How to invoke (primary path)

Use the **Agent tool** with `subagent_type: "codex:codex-rescue"`. Do NOT use the
Skill tool to call Codex (the wrapping skill names are not indexed in every
session). **Never call `codex exec`** — Codex is invoked only through the plugin: the
`codex:codex-rescue` agent (open-ended tasks / diagnosis) or the native `codex review`
reviewer (diff review, or `--uncommitted` critique of written material).

Pass the user's task verbatim in the `prompt` field. The `codex-rescue` agent is
a thin forwarder; it strips routing flags and shapes the prompt itself. Routing
flags the user may include and that you should preserve in the prompt text:

- `--background` / `--wait` — execution mode (default: foreground for small tasks)
- `--model gpt-5.4` | `gpt-5.3-codex` | `gpt-5.3-codex-spark` (or `spark`)
- `--effort high` | `medium` | `low` (default: leave unset)
- `--write` / read-only (default: write-capable unless user wants review only)
- `--resume` / `--fresh` (default: fresh; `--resume` continues prior Codex work)

### Example — single dispatch

```
Agent tool call:
  subagent_type: "codex:codex-rescue"
  description:    "Codex deep-dive on PCS-303 scroll bug"
  prompt:         "Investigate why the middle pane scrolls to the wrong target
                   after inserting a paged block in /apps/web/components/customer-flow/.
                   Reproduce, identify the root cause, propose a fix.
                   Repo: /Users/edevize/Developer/procesarecaietsarcini.
                   --effort high"
```

Return Codex's stdout to the user as-is. Do not paraphrase or summarize unless
asked.

## Review use (review-implementation)

The triple-engine review (`/review-implementation`, `/implement` Role 2) uses ONE Codex
engine — the native `codex review --base <BASE>` (or `--uncommitted`) reviewer — run by
the review orchestrator alongside the Claude comprehensive reviewer and CodeRabbit, then
folded by the synthesizer. No N-pass voting, no shuffled diffs.

## Fallback (only if `codex:codex-rescue` is unavailable)

If the Agent tool reports "Unknown subagent" for `codex:codex-rescue`, use the plugin's
**native reviewer** for review/critique tasks — never `codex exec`:

```bash
codex review --base <BASE>                              # diff review vs a base branch
codex review --uncommitted "<adversarial instructions>" # critique uncommitted work / written material
```

For open-ended diagnosis with no agent path available, surface that to the user rather
than hand-rolling `codex exec` — keeping Codex on the plugin's authenticated runtime is
the doctrine (carries auth, config, and session management).

## Critical evaluation

Codex is a peer, not an authority. If its output contradicts something you know
to be true (recent library APIs, project conventions documented in `CLAUDE.md`),
push back to the user with evidence rather than deferring silently.
