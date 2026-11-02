# Checkpoint 54 — 08:20Z → 09:20Z, 2026-11-02 (v0.2.0 run, hour 10)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0, W1 | Merged (PR #35 `35b7522`, PR #36 `715b1ff`). |
| v0.2 W2 replies without polling (`c5190ea3`) | **PR #37 open, CI running on the latest head** (`bb2764a`). All review, AC, docs and conformance steps are done; the live `claude -p` run resolved AC-v2-206 as `woke` (stream-json). The first CI run failed on one macOS test, which led to the fix cycle below. Merge waits for green CI. |
| W3–W7 | Pending. |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

## Landed on `main` this window

None.

## Decisions

- **macOS CI failure on PR #37.** `a_late_ready_returns_within_the_budget_and_tears_omp_down` (a W1 test) failed with `teardown: Operation not permitted (os error 1)`. Root cause: on macOS, signalling a process group whose remaining members are unreaped zombies returns EPERM until they are reaped, and glen's teardown treated every non-ESRCH error as fatal. This is a latent W1 race that W1's own macOS runs missed. I did not relax the test. Fix cycle:
  - `dec7225`: `Process::stop` treats EPERM as "group still present" and keeps polling (bounded by grace + 2 s); the regression test reaps the zombie member inside the second probe and asserts exactly three probes.
  - `bb2764a`: the same tolerance in the production `terminate` path (`src/omp/bin.rs`), and in the test helpers (`signal_group`, `group_alive`) and the `tests/watch.rs` cleanup, which a Codex review and a local flake of mine had shown share the pattern.
- **Verification.** Claude and Codex reviewed the fix: production logic sound; test-determinism, `watch.rs` and `group_alive` findings were fixed in `bb2764a`. Gate clean; two full `--no-fail-fast` suite runs: 1789 passed, 0 failed; 10-run flake check on `v2_push`: 10 of 10.
- **A process slip, recorded.** One of my chained commands pushed after its test step had failed; nothing merged, but the push started a CI run on a head I had not yet verified green. Future chains gate the push on a clean test exit.
- **AC-v2-206 outcome.** The corrected checker scores the live run `wake=woke` (the completion notification resumed the run after its first `result`). The weak `AC-206-nonce` check, the stream-json-only scope, and a `dontAsk` permission artifact in the resumed turn are recorded in the evidence README.

## Harness friction

- A flake in the omp-compat test family took a CI round trip: the EPERM race was visible only on macOS. Open for W6: confirm the flake-proof workflow covers `omp_compat`, `omp_surface`, `push` and `v2_push`.
- Still open for batch 13: omp's deadline and heading tolerance; a usage limit stops the loop silently; stale `glen` binary fallback; `cargo` not on PATH; the Codex planner's result needs extracting from the handback call; `## Unreleased` and "run it as a workflow" wording in the harness docs; the conformance writer signs commits with the session's model trailer, not the brief's; chained commands should not push after a failed test step.

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys. Open for the owner: whether glen's routing moves from Codex to omp; whether each harness batch and W7 get an upstream-to-agent-os step; the agent-os port session's questions.

## Next hour

1. W2: CI green on PR #37, memory, squash-merge, close the wave.
2. Start W3 (the hidden ACP spike) with a dual-engine step plan.
