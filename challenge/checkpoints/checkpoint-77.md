# Checkpoint 77 — 10:42Z → 11:43Z, 2026-11-03 (v0.2.0 run, hour 35)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0–W5 | Merged (PR #35 `35b7522`, #36 `715b1ff`, #37 `0157285`, #38 `affb069`, #39 `d1708f0`, #40 `582f77b`). |
| v0.2 W6 audits (`0ae5e983`) | **PR #41 open on head `407bf8e` (pushed); AC-602 still open.** This hour: the pty watch-test flake from run 41601896723 was a harness race (`Stream::wait` returned before the stderr reader ran; the pty reader sampled its stop flag after a would-block read), fixed test-side in `dca0801` with a time scale for upper bounds, and the flake-proof job limit raised to 210 minutes in `a6ec24e` (macOS had done 16 clean repeats before the old 120-minute limit cancelled it). My gate and `--no-fail-fast` suite on the head: 2236 passed, 0 failed. New flake-proof run 41621490685 on that head: **macOS failed run 3 of 20** on `ac_v2_403_a_live_endpoint_for_the_session_ends_warmth_within_a_second` (the blind `v2_warm` conformance module, written before the time-scale helper existed); the Linux job is still running. An Implementer is root-causing it from the macOS artifact and sweeping `tests/conformance/v2_warm.rs` and `tests/warm.rs` for the same unscaled-upper-bound class. Left: that fix, my gate and suite, push, a fresh flake-proof dispatch, merge memory (own call), merge, closure. |
| W7 | Pending (release v0.2.0): cherry-pick the held site copy `39aff85`, bump `site/src/data/site.ts` with Cargo.toml, narrow the skill's `allowed-tools` to `ls`, `show`, `tail`, `jobs`, `wait`, `inbox`, `doctor`, `send`. |
| agent-os port (PR #2) | Open, not merged; the port session has no conflicts with the omp-compat points and waits on the owner. |

Progress against the goal: W0–W5 merged; W6 held by the last test-harness flakes; W7 is the release (about 94%).

## Landed on `main` this window

None.

## Decisions

- The AC-602 bar stays at 20 repeats per OS with no product bound widened: upper bounds scale only in flake-check mode, lower bounds and the AC bounds themselves stay exact in plain runs.
- Flake-class ledger so far: MCP test family (pipe end held by a concurrently spawned child, `held_send` before `accepted`), `src/live` unit test (socket peer drop), `dispatch` grandchild pid, pty watch harness (stderr reader and stop-flag races), and now the `v2_warm` blind-suite bounds. Pattern: tests that predate the time scale, or that assume a drop or exit is visible immediately.
- Remaining known timing-based waits left alone: ACP one-tick frame delivery, warm registry `HELD` sleeps, watch `settle()` quiet windows and key sleeps.

## Harness friction

- Each flake-proof round is hours; macOS reports first at about 25 to 30 minutes per failing repeat position, Linux later; logs unreadable until a job finishes.
- The blind conformance suites (written before the time scale) are where the unscaled bounds are; an earlier sweep of tests/ for upper bounds would have saved rounds.
- Implementer commits sometimes carry the Opus trailer; squash messages correct it.
- Carried for batch 13: omp deadline and heading tolerance; stale `glen` binary fallback; `cargo` not on PATH; usage limits idle the run silently; checkpoint commits need a fresh branch after a merge; chained commands must not push after a failed test step; cargo from Python on macOS 27 needs `SDKROOT` unset; `sd` not on PATH, `sed -i` and `cat` blocked by hooks; `gh api .../logs` needs `--allow-escape-sequences` and a strip.

## Blockers

None for the agents. Owner-only: rotate the temporary Cloudflare token and the R2 keys; the agent-os port plan awaits the owner; the Codex MCP leg and interactive Channels check (AC-505c, d); whether to relabel null-job replies. Open for the owner: whether glen's routing moves from Codex to omp; the upstream-to-agent-os step.

## Next hour

1. W6: the `v2_warm` flake fix; my gate and suite; push; re-dispatch flake-proof; merge memory, merge, closure.
2. W7: release v0.2.0 through the release skill.
