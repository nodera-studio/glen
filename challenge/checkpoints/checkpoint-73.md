# Checkpoint 73 — 06:38Z → 07:39Z, 2026-11-03 (v0.2.0 run, hour 31)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0–W5 | Merged (PR #35 `35b7522`, #36 `715b1ff`, #37 `0157285`, #38 `affb069`, #39 `d1708f0`, #40 `582f77b`). |
| v0.2 W6 audits (`0ae5e983`) | **PR #41 open, blocked only on AC-602.** Head `54ee86c` pushed (MCP tests deterministic under flake-check load: two root causes, F14 a test assumption about a pipe end held by a concurrently spawned child, F15 a `held_send` that returned before the job was `accepted`; a shared `GLEN_TEST_TIME_SCALE` helper and a kernel process listing replace `/bin/ps` polling). My quiet gate and `--no-fail-fast` suite on it: 2236 passed, 0 failed. AC-604 acceptance re-run on the fix head: mode p PASS 8 of 8 (`wake=woke`, 6 turns $0.47 plus a resumed 3-turn leg $0.53), mode m PASS 7 of 7 (5 turns $0.22); evidence under `.claude/evidence/v020-w6-acceptance/`, gitleaks clean, no strays. AC-602 flake-proof run 41595037551: **Linux failed on run 3 of 20 with a different test**, the lib unit test `live::client::unary_tests::write_request_cancel_and_deadline_do_not_write_or_drain`; macOS still running. An Implementer is root-causing it (test assumption vs product race in the unary write/cancel/deadline path) and scanning the same class in `src/live`, `src/acp`, `src/rpc`. Left: that fix, my gate and suite, a fresh two-OS flake-proof green (about 100 minutes per dispatch), merge memory, merge, closure. |
| W7 | Pending (release v0.2.0): cherry-pick the held site copy `39aff85` and bump `site/src/data/site.ts` with Cargo.toml in the release PR. |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

Progress against the goal: W0–W5 merged; W6 at PR, held by the load-flake bar; W7 is the release (about 92%).

## Landed on `main` this window

None.

## Decisions

- AC-602 stays a hard bar: each flake-proof failure is root-caused and fixed rather than retried or the bar lowered; product bounds are not widened. Two classes found so far: the MCP test family (fixed) and now a `src/live` unit test.
- Deferred risks recorded from the MCP work: an idle `glen mcp` whose host closed only stdout notices only at its next write (by design); a shutdown between a `glen_send` creating its job and its observer spawning the worker strands the job until readers report it `lost` after `SPAWN_GRACE`.
- Acceptance re-run: real-omp legs (a, b, b3, c1) not re-run in W6, recorded with the reason; Codex legs owner-pending.

## Harness friction

- Each flake-proof dispatch costs about 100 minutes of wall clock and surfaces at most one new failure class at a time; local proof under load (`flake-check.sh N -- <filter>` plus burners) is the only fast loop.
- Implementer commits sometimes carry the Opus trailer; squash messages correct it.
- Carried for batch 13: omp deadline and heading tolerance; stale `glen` binary fallback; `cargo` not on PATH; usage limits idle the run silently; checkpoint commits need a fresh branch after a merge; chained commands must not push after a failed test step; cargo from Python on macOS 27 needs `SDKROOT` unset; `sd` not on PATH, `sed -i` and `cat` blocked by hooks; `gh api .../logs` needs `--allow-escape-sequences` and a strip.

## Blockers

None for the agents. Owner-only: rotate the temporary Cloudflare token and the R2 keys; the S-01 decision and threat-model question; the Codex MCP leg and interactive Channels check (AC-505c, d); whether to relabel null-job replies. Open for the owner: whether glen's routing moves from Codex to omp; the upstream-to-agent-os step; the agent-os port session's questions.

## Next hour

1. W6: the `src/live` flake fix; my gate and suite; re-dispatch flake-proof (green on both OSes); merge memory, merge, closure.
2. W7: release v0.2.0 through the release skill (preflight re-runs acceptance on the final `main`).
