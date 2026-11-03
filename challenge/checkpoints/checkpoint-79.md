# Checkpoint 79 — 12:44Z → 13:45Z, 2026-11-03 (v0.2.0 run, hour 37)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0–W5 | Merged (PR #35 `35b7522`, #36 `715b1ff`, #37 `0157285`, #38 `affb069`, #39 `d1708f0`, #40 `582f77b`). |
| v0.2 W6 audits (`0ae5e983`) | **PR #41 open; AC-602 still open.** Pushed head `7a8655b` carries two root-caused test fixes from the previous round: `b232c01` (test harnesses wait for glen's exit, not a pipe's end of file: on macOS std creates a pipe and sets FD_CLOEXEC only after, so a concurrently spawned child holds the write end and `Command::output()` can block for its lifetime; it made `ac_v2_403…` see its endpoint after the warm window expired) and `7a8655b` (the ACP cancel cases signalled on a `session/load` replay chunk instead of the turn's chunk). Product code unchanged. My gate and `--no-fail-fast` suite on `7a8655b`: 2238 passed, 0 failed. flake-proof run 41641481889 on that head: **macOS failed run 4 of 20** on `cycle3_final_identity_check_after_deadline_or_cancel_detects_silent_fork` (a test outside the earlier sweep list); Linux still running. The sweep Implementer has committed `a837ace` (tests/acp.rs) so far and is now widening to every remaining file under `tests/` with unscaled upper bounds, pipe-EOF waits or sleep-ordered readiness. Left: that sweep, my gate and suite, push, a fresh flake-proof dispatch, merge memory (own call), merge, closure. |
| W7 | Pending (release v0.2.0): cherry-pick the held site copy `39aff85`, bump `site/src/data/site.ts` with Cargo.toml, narrow the skill's `allowed-tools` to `ls`, `show`, `tail`, `jobs`, `wait`, `inbox`, `doctor`, `send`. |
| agent-os port (PR #2) | Open, not merged; the port session has no conflicts with the omp-compat points and waits on the owner. |

Progress against the goal: W0–W5 merged; W6 held by the last test-harness flakes; W7 is the release (about 94%).

## Landed on `main` this window

None.

## Decisions

- Bar unchanged: 20 repeats per OS, no product bound widened, AC bounds exact in plain runs. Every CI failure so far across five rounds has been a test-harness defect, never a product defect; the sweep is therefore widened to all of `tests/`.
- Deferred product hardening (not W6 scope, to record as tech debt): the detached warm worker inherits any non-close-on-exec fds from its caller and holds them for up to 10 minutes; closing fds above 2 in its `pre_exec` would harden it.
- Durable fact for Mnemosyne: on macOS, std pipes get FD_CLOEXEC only after creation, so test harnesses must capture child output in files, not read pipes to end of file.

## Harness friction

- Five flake-proof rounds, each one exposing the next unswept module, because the fix was scoped to the test CI named. The wide sweep is the correction; cost so far about 10 hours of wall clock.
- Implementer commits sometimes carry the Opus trailer; squash messages correct it.
- Carried for batch 13: omp deadline and heading tolerance; stale `glen` binary fallback; `cargo` not on PATH; usage limits idle the run silently; checkpoint commits need a fresh branch after a merge; chained commands must not push after a failed test step; cargo from Python on macOS 27 needs `SDKROOT` unset; `sd` not on PATH, `sed -i` and `cat` blocked by hooks; `gh api .../logs` needs `--allow-escape-sequences` and a strip; the conformance-writer prompt should require scaling every upper bound through the flake helper.

## Blockers

None for the agents. Owner-only: rotate the temporary Cloudflare token and the R2 keys; the agent-os port plan awaits the owner; the Codex MCP leg and interactive Channels check (AC-505c, d); whether to relabel null-job replies. Open for the owner: whether glen's routing moves from Codex to omp; the upstream-to-agent-os step.

## Next hour

1. W6: the all-of-tests sweep lands; my gate and suite; push; re-dispatch flake-proof; merge memory, merge, closure.
2. W7: release v0.2.0 through the release skill.
