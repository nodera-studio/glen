# Checkpoint 64 — 21:18Z → 22:19Z, 2026-11-02 (v0.2.0 run, hour 22)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0–W3 | Merged (PR #35 `35b7522`, #36 `715b1ff`, #37 `0157285`, #38 `affb069`). |
| v0.2 W4 warm worker (`36b5d618`) | **Merged** (PR #39 `d1708f0`); board `done` + `shipped:main`, queue item done, branch deleted, index refresh started, orphan sweep done. |
| v0.2 W5 glen mcp (`37706789`) | **Planning started**: Claude and Codex planners dispatched at 22:17Z (branch `feat/v020-wave-5-mcp`). Left: anonymize, blind judge, judge edits, Implementer, review gate, docs, conformance, PR. |
| W6–W7 | Pending. |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

Progress against the goal: plan plus five of eight waves done (about 60%).

## Landed on `main` this window

- PR #39 `d1708f0` — W4: opt-in warm worker (`--warm`, `glen release`, steal-from-queue), registry metadata lock, honest release/teardown failure reporting; 2054 tests pass, CI green on macOS and Linux.

## Decisions

- W4 merged with its documented residuals (release before a worker's first registry publish; a stopped metadata-lock holder can leave files until its release). Memory `bdebb4a7`.
- Killed a leaked `fake_omp --version` process (about 9.7 h old, parent init, predating this wave); source not traced.
- W5 planning retries the Codex planner (it stalled on all three W4 attempts); fallback is a single-candidate judge.

## Harness friction

- Codex planner stalls on long-read planning prompts (three on W4): retried for W5 with a steady-output instruction.
- Checkpoint commits ride the wave branch, so a checkpoint written after a merge needs a fresh branch (`main` takes no direct pushes) and a post-PR push restarts CI.
- Carried for batch 13: omp deadline and heading tolerance; stale omp result file reads "failed" mid-run; stale `glen` binary fallback; `cargo` not on PATH; Implementer commit trailers sometimes say Opus; usage limits idle the run silently; chained commands must not push after a failed test step; cargo from Python on macOS 27 needs `SDKROOT` unset; `sd` not on PATH, `sed -i` and `cat` blocked by hooks.

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys. Open for the owner: whether glen's routing moves from Codex to omp; the upstream-to-agent-os step; the agent-os port session's questions.

## Next hour

1. W5: both plans back, anonymize, blind judge (or single-candidate fallback), judge edits, Implementer steps.
2. Keep the Implementer to the plan's step groups; my gate and suite with no other agents running.
