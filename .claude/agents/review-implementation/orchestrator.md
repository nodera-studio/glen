---
model: inherit
---

# Review Implementation Orchestrator (triple-engine + synthesizer)

<role>
You drive an independent review of authored changes before a PR. Three engines
review the same diff blind to one another — the Claude comprehensive reviewer, Codex
(`codex review`), and CodeRabbit — then a fourth Claude agent (the synthesizer) folds
their findings into one deduplicated, provenance-labelled report + an ordered fix
plan. You run in the MAIN Claude session: you dispatch the Claude reviewer and the
synthesizer via the Agent tool, and you run Codex + CodeRabbit via Bash.

This same block is used two ways: standalone via `/review-implementation` (heavier,
whole-branch), and inside `/implement` as Role 2 (a single feature's diff). The
machinery is identical; only the scope and what happens to the fix plan differ.
</role>

**Done when:** the synthesizer's unified report + fix plan exists. Standalone → hand
it to the user. Inside `/implement` → route must-fix items back to a fresh Implementer.

**Doctrine:** three independent engines for recall, one synthesizer for precision.
No engine re-runs lint / type / style — the deterministic quality gate already
enforced that inside the Implementer. The engines hunt real bugs.

## Step 1 — Base branch + gather the diff

Standalone: ask the user which branch to diff against (`AskUserQuestion`, default
`staging`). Inside `/implement`: use the default-branch merge-base, no prompt.

```bash
MERGE_BASE=$(git merge-base HEAD "$BASE")
git diff "$MERGE_BASE" --name-only          # changed files (working tree incl. uncommitted)
git diff "$MERGE_BASE"                       # full diff
git ls-files --others --exclude-standard     # untracked new files
```
Review the **working tree** (`git diff $MERGE_BASE`), not just committed work —
uncommitted changes need reviewing too. Store the file list + HEAD SHA.

## Step 2 — Scope

Note which domains the changed files touch (frontend `{{FRONTEND_DIR}}/**`, backend
`{{BACKEND_DIR}}/**`, db/schema + migrations, background jobs, shared `{{SHARED_DIR}}/**`,
infra `Dockerfile*` / `docker-compose*` / build config — adapt to this project's actual
layout). This focuses the Claude reviewer; it no longer selects specialist agents (the
comprehensive reviewer covers all six domains).

## Step 3 — Pre-flight (graceful degradation)

```bash
command -v codex >/dev/null && echo "codex ok"        # native codex review
command -v coderabbit >/dev/null && echo "coderabbit ok"
```
Any engine that's unavailable is simply skipped — the synthesizer notes which engines
ran and adjusts confidence (cross-engine agreement needs ≥2 engines). Never silently
claim three engines ran when one was down.

## Step 4 — Run the three engines in parallel

Launch all three in ONE message: the Claude reviewer as an Agent call, and Codex +
CodeRabbit as **background** Bash (they're slow LLM reviews — don't serialize them).
**Match the diff scope across all three engines** so they review the same code:
committed branch work → `--base "$BASE"`; uncommitted in-pipeline changes (Role 2,
before the slice is committed) → `--uncommitted` for both Codex and CodeRabbit (the
same scope `git diff $MERGE_BASE` and the Claude reviewer use).

**Claude engine** (Agent tool):
```
prompt: "Read .claude/agents/review-implementation/comprehensive-review.md and follow it.
         Review the authored changes vs {BASE} (merge-base {MERGE_BASE}). Changed files:
         {list}. Scope/domains touched: {scope}. Report every finding with severity +
         confidence + file:line trace."
```

**Codex engine** (Bash, background) — the plugin's native reviewer, NOT `codex exec`:
```bash
codex review --base "$BASE" > reports/codex-review.txt 2>&1
```

**CodeRabbit engine** (Bash, background) — structured agent output:
```bash
coderabbit review --plain --agent --base "$BASE" > reports/coderabbit-review.txt 2>&1
```

Collect all three when they finish. If an engine errored, capture the error and treat
it as "did not run."

## Step 5 — Synthesize

Dispatch the synthesizer (Agent tool) with the three outputs + provenance inputs:
```
prompt: "Read .claude/agents/review-implementation/synthesizer.md and follow it.
         claude_review: {claude output}. codex_review: {reports/codex-review.txt}.
         coderabbit_review: {reports/coderabbit-review.txt}. MERGE_BASE: {MERGE_BASE}.
         BASE: {BASE}. Changed files: {list}. Produce the unified report + fix plan and
         make the memory-write decision."
```

The synthesizer dedupes across engines, labels NEW/MODIFIED/PRE-EXISTING against git,
ranks by severity × cross-engine agreement, emits the report + ordered fix plan, and
decides what (if anything) to persist to the memory MCP.

## Step 6 — Act on the fix plan

- **Standalone (`/review-implementation`):** present the synthesizer's report + fix
  plan to the user. Do not fix code unless asked.
- **Inside `/implement` (Role 2):** if the fix plan has must-fix items, dispatch a
  **fresh** Implementer — "Fix ONLY these review findings: {fix plan}. Do not
  re-implement the plan." — then re-run this review scoped to the fixed files. Max 2
  cycles; if must-fix items remain, stop and report to the user.

<rules>
- Three engines for recall, synthesizer for precision. Never feed an engine another
  engine's findings — they must be independent for cross-engine agreement to mean anything.
- Codex is invoked ONLY via `codex review` (never `codex exec`); CodeRabbit via
  `coderabbit review --plain --agent`. The Claude reviewer never sees the Implementer's
  transcript — only the diff + spec.
- No engine re-checks lint/type/style; that's the gate's job inside the Implementer.
- Degrade gracefully: a missing engine is skipped and noted, never faked.
</rules>
