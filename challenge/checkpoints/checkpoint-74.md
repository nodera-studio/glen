# Checkpoint 74 — 07:39Z → 08:40Z, 2026-11-03 (v0.2.0 run, hour 32)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0–W5 | Merged (PR #35 `35b7522`, #36 `715b1ff`, #37 `0157285`, #38 `affb069`, #39 `d1708f0`, #40 `582f77b`). |
| v0.2 W6 audits (`0ae5e983`) | **PR #41 open on head `fe74ef8`, waiting on AC-602.** The second flake class (the Linux failure `live::client::unary_tests::write_request_cancel_and_deadline_do_not_write_or_drain`) was a test assuming that dropping a socket peer closes it; a child another test thread spawns holds an inherited copy between fork and exec. Fixed test-side with a held-open peer and an own-side shutdown, plus every unscaled product upper bound in `src/live`, `src/acp` and `src/rpc/warm.rs` now scaled by `GLEN_TEST_TIME_SCALE`; a third flake (a grandchild writing its pid after `yes` started) fixed too. Local proof: full `--lib` 20 of 20 under heavy load. My gate and `--no-fail-fast` suite on `fe74ef8`: 2236 passed, 0 failed. PR CI: macOS and Linux `check`, `linux-sandbox`, `msrv`, `site` all green. flake-proof dispatch 41601896723 (both OSes) is in progress, about 100 minutes per dispatch. Left: that run green on both OSes, the merge memory (own call), merge, board and queue closure, index refresh, orphan sweep. |
| W7 | Pending (release v0.2.0): cherry-pick the held site copy `39aff85` and bump `site/src/data/site.ts` with Cargo.toml in the release PR. |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

Progress against the goal: W0–W5 merged; W6 waiting only on the flake proof; W7 is the release (about 93%).

## Landed on `main` this window

None.

## Decisions

- AC-602 stays a hard bar; every failure so far was a real test defect, never retried away. Three test classes fixed (MCP family, `src/live` unit test, `dispatch` grandchild pid).
- Three timing assumptions were left alone because fixing them needs a product test seam, not scaling: an ACP test expecting one 100 ms tick to deliver a queued frame, the warm registry `HELD` sleeps assuming the metadata lock is released within 1 s, and two EOF tests whose 5–10 s waits absorb inherited-copy delays. If flake-proof fails on one of these, it gets a seam.
- A flake-proof failure is treated as a finding to fix, not noise; the agent proves each fix locally under load before a new dispatch.

## Harness friction

- Each flake-proof dispatch takes about 100 minutes and its logs are unavailable until a job finishes, so progress cannot be watched run by run.
- Implementer commits sometimes carry the Opus trailer; squash messages correct it.
- Carried for batch 13: omp deadline and heading tolerance; stale `glen` binary fallback; `cargo` not on PATH; usage limits idle the run silently; checkpoint commits need a fresh branch after a merge; chained commands must not push after a failed test step; cargo from Python on macOS 27 needs `SDKROOT` unset; `sd` not on PATH, `sed -i` and `cat` blocked by hooks; `gh api .../logs` needs `--allow-escape-sequences` and a strip.

## Blockers

None for the agents. Owner-only: rotate the temporary Cloudflare token and the R2 keys; the S-01 decision and threat-model question; the Codex MCP leg and interactive Channels check (AC-505c, d); whether to relabel null-job replies. Open for the owner: whether glen's routing moves from Codex to omp; the upstream-to-agent-os step; the agent-os port session's questions.

## Next hour

1. W6: flake-proof green on both OSes (or the next fix), merge memory, merge, closure.
2. W7: release v0.2.0 through the release skill.
