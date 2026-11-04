# Checkpoint 78 — 11:43Z → 12:44Z, 2026-11-03 (v0.2.0 run, hour 36)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0–W5 | Merged (PR #35 `35b7522`, #36 `715b1ff`, #37 `0157285`, #38 `affb069`, #39 `d1708f0`, #40 `582f77b`). |
| v0.2 W6 audits (`0ae5e983`) | **PR #41 open on head `407bf8e` (pushed); AC-602 still open.** flake-proof run 41621490685 finished: **Linux ran 17 clean repeats, then failed run 18** on `v2_acp::ac_v2_302_an_unconfirmed_cancel_still_exits_130_within_the_bound` (the W3 blind suite, a bound of about 9 s); **macOS failed run 3** on `ac_v2_403_a_live_endpoint_for_the_session_ends_warmth_within_a_second` (the W4 blind suite). Both are blind conformance modules written before the time-scale helper, with unscaled upper bounds. An Implementer is root-causing both from the artifacts and sweeping all the blind suites (`v2_acp`, `v2_warm`, `v2_push`, `v2_compat`, `v2_w0`, plus `tests/push.rs`, `omp_compat.rs`, `acp.rs`, `warm.rs`); its edits to the shared test helpers are in the working tree, uncommitted. PR CI green; my gate and suite on the pushed head: 2236 passed, 0 failed. Left: that sweep, my gate and suite, push, a fresh flake-proof dispatch, merge memory (own call), merge, closure. |
| W7 | Pending (release v0.2.0): cherry-pick the held site copy `39aff85`, bump `site/src/data/site.ts` with Cargo.toml, narrow the skill's `allowed-tools` to `ls`, `show`, `tail`, `jobs`, `wait`, `inbox`, `doctor`, `send`. |
| agent-os port (PR #2) | Open, not merged; the port session has no conflicts with the omp-compat points and waits on the owner. |

Progress against the goal: W0–W5 merged; W6 held by the last unscaled blind-suite bounds; W7 is the release (about 94%).

## Landed on `main` this window

None.

## Decisions

- The bar is unchanged: 20 repeats per OS, no product bound widened, AC bounds exact in plain runs.
- Change of method: after three rounds each finding one new blind-suite bound, the fix is now a full sweep of every blind conformance module and integration test for unscaled upper bounds and setup waits, with a before/after load proof per module, instead of fixing the one test CI named.
- Linux is now at 17 of 20 clean in one run (up from 12), and macOS has cleared the harness classes; both remaining failures are in the blind suites.

## Harness friction

- The blind conformance writers did not use the time-scale helper because it did not exist when they ran; a rule in the conformance-writer prompt ("scale every upper bound through the flake helper once it exists") would prevent this class (log for batch 13).
- Each flake-proof round is hours; logs are unreadable until a job completes.
- Implementer commits sometimes carry the Opus trailer; squash messages correct it.
- Carried for batch 13: omp deadline and heading tolerance; stale `glen` binary fallback; `cargo` not on PATH; usage limits idle the run silently; checkpoint commits need a fresh branch after a merge; chained commands must not push after a failed test step; cargo from Python on macOS 27 needs `SDKROOT` unset; `sd` not on PATH, `sed -i` and `cat` blocked by hooks; `gh api .../logs` needs `--allow-escape-sequences` and a strip.

## Blockers

None for the agents. Owner-only: rotate the temporary Cloudflare token and the R2 keys; the agent-os port plan awaits the owner; the Codex MCP leg and interactive Channels check (AC-505c, d); whether to relabel null-job replies. Open for the owner: whether glen's routing moves from Codex to omp; the upstream-to-agent-os step.

## Next hour

1. W6: the blind-suite sweep lands; my gate and suite; push; re-dispatch flake-proof; merge memory, merge, closure.
2. W7: release v0.2.0 through the release skill.
