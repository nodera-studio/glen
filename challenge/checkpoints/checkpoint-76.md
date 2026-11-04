# Checkpoint 76 — 09:41Z → 10:42Z, 2026-11-03 (v0.2.0 run, hour 34)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0–W5 | Merged (PR #35 `35b7522`, #36 `715b1ff`, #37 `0157285`, #38 `affb069`, #39 `d1708f0`, #40 `582f77b`). |
| v0.2 W6 audits (`0ae5e983`) | **PR #41 open; AC-602 still open.** Result of flake-proof run 41601896723 on `fe74ef8`: **macOS did not fail**: 16 clean repeats, then the job hit its 120-minute `timeout-minutes` and was cancelled (macOS takes about 8 minutes per repeat, so 20 repeats need about 160 minutes). Fixed in `a6ec24e` (limit 210 minutes, with the measured timings in a comment; not yet pushed). **Linux failed run 13 of 20** on the pty conformance test `watch::ac505_a_vanished_file_shows_tail_fs_not_found_in_danger_and_stops`; an Implementer is root-causing it and fixing the same class in `tests/watch.rs`, `tests/conformance/watch.rs` and `tests/conformance/reader.rs` (uncommitted in the working tree). Left: that fix, my gate and suite, push, a fresh flake-proof dispatch (Linux about 100 minutes, macOS about 160), the merge memory (own call), merge, closure. |
| W7 | Pending (release v0.2.0): cherry-pick the held site copy `39aff85`, bump `site/src/data/site.ts` with Cargo.toml, narrow the skill's `allowed-tools` to `ls`, `show`, `tail`, `jobs`, `wait`, `inbox`, `doctor`, `send`. |
| agent-os port (PR #2) | Open, not merged; the port session has no conflicts with the omp-compat points and waits on the owner. |

Progress against the goal: W0–W5 merged; W6 held by the last pty flake and the next proof round; W7 is the release (about 94%).

## Landed on `main` this window

None.

## Decisions

- A cancelled flake-proof job is not a failed one: the macOS result was 16 of 16 clean before an arithmetic problem in the job limit, which I fixed by measurement rather than by cutting repeats.
- The proof stays at 20 repeats per OS; no bound was widened to get a pass.

## Harness friction

- The flake-proof job limit (120 minutes) was smaller than the proof itself on macOS; the signal was a "cancelled" conclusion, easy to misread as a failure.
- Each dispatch is a multi-hour loop; logs are unreadable until a job completes.
- Implementer commits sometimes carry the Opus trailer; squash messages correct it.
- Carried for batch 13: omp deadline and heading tolerance; stale `glen` binary fallback; `cargo` not on PATH; usage limits idle the run silently; checkpoint commits need a fresh branch after a merge; chained commands must not push after a failed test step; cargo from Python on macOS 27 needs `SDKROOT` unset; `sd` not on PATH, `sed -i` and `cat` blocked by hooks; `gh api .../logs` needs `--allow-escape-sequences` and a strip.

## Blockers

None for the agents. Owner-only: rotate the temporary Cloudflare token and the R2 keys; the agent-os port plan awaits the owner; the Codex MCP leg and interactive Channels check (AC-505c, d); whether to relabel null-job replies. Open for the owner: whether glen's routing moves from Codex to omp; the upstream-to-agent-os step.

## Next hour

1. W6: the watch flake fix; my gate and suite; push; re-dispatch flake-proof; then merge memory, merge, closure.
2. W7: release v0.2.0 through the release skill.
