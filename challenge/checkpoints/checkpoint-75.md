# Checkpoint 75 — 08:40Z → 09:41Z, 2026-11-03 (v0.2.0 run, hour 33)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0–W5 | Merged (PR #35 `35b7522`, #36 `715b1ff`, #37 `0157285`, #38 `affb069`, #39 `d1708f0`, #40 `582f77b`). |
| v0.2 W6 audits (`0ae5e983`) | **PR #41 open on head `fe74ef8`; AC-602 still open.** The flake-proof run 41601896723 is the best result yet: Linux passed runs 1–12 of 20, then **failed run 13** on `watch::ac505_a_vanished_file_shows_tail_fs_not_found_in_danger_and_stops` (a pty-driven conformance test); the macOS job is still running. An Implementer is root-causing it (test timing assumption vs a product race in the TUI's vanished-file handling) and scanning the rest of the pty-driven watch tests for the same shape. PR CI is green; my gate and suite on `fe74ef8`: 2236 passed, 0 failed. Left: that fix, my gate and suite, another flake-proof dispatch (about 100 minutes), the merge memory (own call), merge, closure. |
| W7 | Pending (release v0.2.0): cherry-pick the held site copy `39aff85`, bump `site/src/data/site.ts` with Cargo.toml, and narrow the skill's `allowed-tools` to `ls`, `show`, `tail`, `jobs`, `wait`, `inbox`, `doctor`, `send` (owner-agreed direction; `doctor` approval warning and `GLEN_OMP_APPROVAL` passthrough held as omp-policy side). |
| agent-os port (PR #2) | Open, not merged; the port session replied that the omp-compat points do not conflict with PR #2 and is waiting on the owner. |

Progress against the goal: W0–W5 merged; W6 held by the last load-flake class; W7 is the release (about 94%).

## Landed on `main` this window

None.

## Decisions

- The flake-proof bar stays: a failure is root-caused, never retried away. Each dispatch now gets further (macOS and Linux both cleared the earlier classes; Linux ran 12 clean repeats before a new, different test failed), so the remaining classes are the long tail of pty and timing tests.
- Owner direction recorded: omp approval policy is not glen's job (omp profile config); glen narrows its own skill pre-approval and documents the consequence; agent-os must stay omp-compatible, and the requirements were sent to the port session.

## Harness friction

- Each dispatch exposes one new failure per OS at most, and its logs are unreadable until a job completes; the proof loop is about 100 minutes per round.
- Implementer commits sometimes carry the Opus trailer; squash messages correct it.
- Carried for batch 13: omp deadline and heading tolerance; stale `glen` binary fallback; `cargo` not on PATH; usage limits idle the run silently; checkpoint commits need a fresh branch after a merge; chained commands must not push after a failed test step; cargo from Python on macOS 27 needs `SDKROOT` unset; `sd` not on PATH, `sed -i` and `cat` blocked by hooks; `gh api .../logs` needs `--allow-escape-sequences` and a strip.

## Blockers

None for the agents. Owner-only: rotate the temporary Cloudflare token and the R2 keys; the agent-os port plan awaits the owner; the S-01 follow-ups above; the Codex MCP leg and interactive Channels check (AC-505c, d); whether to relabel null-job replies. Open for the owner: whether glen's routing moves from Codex to omp; the upstream-to-agent-os step.

## Next hour

1. W6: the watch flake fix; my gate and suite; flake-proof re-dispatch; merge memory, merge, closure.
2. W7: release v0.2.0 through the release skill.
