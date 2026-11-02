# Checkpoint 51 — 04:53Z → 06:17Z, 2026-11-02 (v0.2.0 run, hour 7)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0 foundation | Merged (PR #35, `35b7522`). |
| v0.2 W1 omp compat | **Merged** (PR #36, `715b1ff`); board done + `shipped:main`; memory `ce11b9f9`. |
| v0.2 W2 replies without polling (`c5190ea3`) | **Implementing** on `feat/v020-wave-2-push`; the Implementer has committed steps 1–4 (`244e704`). Plan judged (Claude spine, 13 steps, judge edits E1–E8, three Codex grafts). Step 11 (blind conformance module) and Step 13 (live `claude -p` smoke) are mine. |
| W3–W7 | Pending. |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

## Landed on `main` this window

- PR #36 → `715b1ff`: W1, omp version range, `doctor --probe`, nightly latest-omp contract. Review: Claude ‖ Codex ‖ omp. One real AC gap found by the blind conformance suite and fixed (`rpc_protocol` recorded before host-tool registration). 1694 tests pass; CI green on every job including the nightly `omp-latest` job.

## Decisions

- **W2 plan.** Dual planners and a blind judge chose the Claude plan as the spine, with three Codex grafts: a complete-line reader for the reply watch, a terminal recheck before publishing a reply wake, and a wrapper pass-through test. The judge dropped the Codex plan's extra locking on the inbox read marker (a race there duplicates a delivery, never loses one, and callers dedupe by `id`). An unknown `--after` id is a usage error.
- **W2 AC changes**, all judged justified: the 0/6/7/11 terminal mapping; replies carried in the terminal documents; an unknown `--after` id as exit 2 `usage`; "no inbox-loop instruction" made testable; the live `claude -p` run owned by the orchestrator with a CONDITIONAL on its evidence.
- **omp review.** On the W1 fix reviews, omp returned "No findings" on the last pass but no vote (its output had a preamble). Its first two W1 runs timed out at 15 minutes with nothing.

## Harness friction

- The omp engine times out on large diffs, and discards a good review that has a text preamble. Open for batch 13.
- A usage limit stops the whole loop silently. Open for batch 13: write a `blocked: usage limit` note.
- Still open for batch 13: staging and launching the review gate in separate steps; the stale `glen` binary fallback; `cargo` not on PATH; the Codex planner's result needs extracting from the handback call; the `## Unreleased` and "run it as a workflow" wording in the harness docs.

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys. Open for the owner: whether glen routing moves from Codex to omp; whether each harness batch and W7 get an upstream-to-agent-os step; the agent-os port session's questions.

## Next hour

1. W2: the Implementer finishes Steps 5–10 and 12; then my deliberate re-run, review (Claude ‖ Codex ‖ omp), AC check, docs ‖ conformance writer, the live `claude -p` smoke, PR, merge.
2. Start W3 (ACP spike) with a dual-engine step plan.
