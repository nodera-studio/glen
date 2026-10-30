---
model: inherit
---

# Review Implementation Orchestrator (triple-engine + synthesizer)

<role>
You drive an independent review of authored changes before a PR. Three engines
review the same diff blind to one another — the Claude comprehensive reviewer, Codex
(`codex review`), and omp (a Cursor model run read-only through glen,
`.claude/scripts/omp-engine.sh`) — then a fourth Claude agent (the synthesizer) folds
their findings into one deduplicated, provenance-labelled report + an ordered fix
plan. You run in the MAIN Claude session: you dispatch the Claude reviewer and the
synthesizer via the Agent tool, and you run Codex + omp via Bash.

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

Standalone: diff against `main` (glen's only long-lived branch) unless the user names
another base. Inside `/implement`: use the `main` merge-base, no prompt.

```bash
MERGE_BASE=$(git merge-base HEAD "$BASE")
git diff "$MERGE_BASE" --name-only          # changed files (working tree incl. uncommitted)
git diff "$MERGE_BASE"                       # full diff
git ls-files --others --exclude-standard     # untracked new files
```
Review the **working tree** (`git diff $MERGE_BASE`), not just committed work —
uncommitted changes need reviewing too. Store the file list + HEAD SHA.

## Step 2 — Scope

Note which areas the changed files touch: the CLI entry and output (`src/main.rs` and
command handlers), omp file discovery and parsing, omp RPC / bridge I/O (all under
`src/`), tests (`tests/`, `tests/fixtures/`), and build/CI config (`Cargo.toml`,
`Cargo.lock`, `.github/workflows/ci.yml`, `lefthook.yml`). glen has no frontend,
database, background jobs or Docker. This focuses the Claude reviewer; it no longer selects specialist agents (the
comprehensive reviewer covers all six domains).

## Step 3 — Pre-flight (graceful degradation)

```bash
command -v codex >/dev/null && echo "codex ok"        # native codex review
bash .claude/scripts/omp-engine.sh probe             # exit 0 = ready, exit 3 = skipped
```

When the probe is skipped, expect a two-engine run (Claude ‖ Codex) and put the probe's
`.reason` from its JSON stdout line in the report.
Any engine that's unavailable is simply skipped — the synthesizer notes which engines
ran and adjusts confidence (cross-engine agreement needs ≥2 engines). Never silently
claim three engines ran when one was down.

## Step 4 — Run the three engines in parallel

Launch all three in ONE message: the Claude reviewer as an Agent call, and Codex +
omp (when the probe is ready) as **background** Bash (they're slow LLM reviews — don't serialize them).
**Match the diff scope across all three engines** so they review the same code:
committed branch work → `--base "$BASE"`; uncommitted in-pipeline changes (Role 2,
before the slice is committed) → `--uncommitted` for Codex and the omp engine (use
`git diff HEAD` plus untracked files for the Claude reviewer in that scope).

**Claude engine** (Agent tool):
```
prompt: "Read .claude/agents/review-implementation/comprehensive-review.md and follow it.
         Review the authored changes vs {BASE} (merge-base {MERGE_BASE}). Changed files:
         {list}. Scope/domains touched: {scope}. Report every finding with severity +
         confidence + file:line trace."
```

**Codex engine** (Bash, background) — the plugin's native reviewer, NOT `codex exec`:
```bash
mkdir -p target/reports && codex review --base "$BASE" > target/reports/codex-review.txt 2>&1
```

**omp engine** (Bash, background) — only when the probe exits 0:
```bash
mkdir -p target/reports && bash .claude/scripts/omp-engine.sh review --repo "$PWD" --base "$BASE" --out target/reports/omp-review.txt > target/reports/omp-engine.log 2>&1
```
Use `--uncommitted` in place of `--base "$BASE"` for the uncommitted-scope role.
Exit codes: 0 ok, 3 skipped, 4 failed, 5 timeout, 2 usage error. Read the answer in
`target/reports/omp-review.txt` and the result in `target/reports/omp-review.txt.json`.
Do NOT re-run omp manually: the engine already retries capacity issues once internally.
Recover a killed run from that JSON's `.job` and `.glen_home` while `.status` is
`"running"`: `GLEN_HOME=<that glen_home> glen wait <job>`, never by re-sending. Use the
scrubbed recovery environment documented in `.claude/skills/omp/SKILL.md`.

Collect all three when they finish. If Codex failed on capacity or rate ("Selected model
is at capacity", 429, rate limit), re-run it once after a short wait (about 60 s). If an
engine errored (or the Codex retry failed too), capture the error and treat it as "did
not run": the synthesizer works from the remaining engines and the report names the
missing one.
For omp, any non-zero exit, or an exit-0 result with `"status":"failed"` and
`"reason":"malformed output"`, means "omp did not run"; pull `.reason` from
`target/reports/omp-review.txt.json` for the report (or use the probe's reason if skipped
before dispatch). No non-`ok` omp status contributes a vote.

Engine outputs keep engine-qualified names (`codex-review.txt`, `omp-review.txt`,
and `claude-review.md` if you save the Claude pass), never a bare `claude.md`: on macOS's
case-insensitive filesystem Claude Code loads any `claude.md` as a nested `CLAUDE.md`.

When the diff touches rendered output (`site/`, a TUI, terminal rendering), add to the
Claude engine's prompt: build and run it and look at the output (browser screenshot, or
CLI/TUI output under a pty) before calling it correct; a code trace alone is not enough.
omp traces code only — it cannot run or observe UI/runtime output.

## Step 5 — Synthesize

Dispatch the synthesizer (Agent tool) with the three outputs + provenance inputs:
```
prompt: "Read .claude/agents/review-implementation/synthesizer.md and follow it.
         claude_review: {claude output}. codex_review: {target/reports/codex-review.txt}.
         omp_review: {target/reports/omp-review.txt}.
         omp_status: {.status}, omp_reason: {.reason}, omp_model: {.model},
         omp_tokens: {.usage.totalTokens} from target/reports/omp-review.txt.json
         (if probe-skipped, pass its status/reason and no omp review).
         MERGE_BASE: {MERGE_BASE}.
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
  cycles; if must-fix items remain, stop and report to the user. Give each finding its
  exact line range (`implementation/orchestrator.md` → Role 2 → "Line-range fix briefs").

<rules>
- Three engines for recall, synthesizer for precision. Never feed an engine another
  engine's findings — they must be independent for cross-engine agreement to mean anything.
- Codex is invoked ONLY via `codex review` (never `codex exec`); omp ONLY via
  `.claude/scripts/omp-engine.sh` (never `omp` directly, never a bare `glen send`, never
  the default omp profile). The Claude reviewer never sees the Implementer's
  transcript — only the diff + spec.
- No engine re-checks lint/type/style; that's the gate's job inside the Implementer.
- Degrade gracefully: a missing engine is skipped and noted, never faked.
</rules>
