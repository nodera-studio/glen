# Checkpoint 68 — 01:22Z → 02:23Z, 2026-11-03 (v0.2.0 run, hour 26)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0–W4 | Merged (PR #35 `35b7522`, #36 `715b1ff`, #37 `0157285`, #38 `affb069`, #39 `d1708f0`). |
| v0.2 W5 glen mcp (`37706789`) | **Code, reviews, docs and blind conformance all landed on `feat/v020-wave-5-mcp`** (head `f486d13`, not pushed). Review round 1 (12 items) and round 2 (Codex: one release per call so a replacement call is never deleted, a disarmed test reap guard, doctor ready after its SIGINT trap) are fixed. Blind conformance: 64 cases in `tests/conformance/v2_mcp.rs`, all pass, no product defect found. My last gate and suite (at `1e61fd2`): 2136 passed, 0 failed; the final-head run is next. Left: my gate and suite on `f486d13`, live legs (AC-505b/c), PR, CI (Linux-only code needs a green ubuntu run), memory, merge. |
| W6–W7 | Pending. |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

## Landed on `main` this window

None.

## Decisions

- Conformance writer's plan-versus-reality notes accepted: `show`/`tail` carry a clock-relative `age_ms`, so byte-equality compares after neutralising only that field. Logged for W6: with stdout unread and a 600-request backlog, EOF shutdown took 2.1–2.9 s on an idle machine because queued requests are answered before EOF is seen (a signal takes about 1 s); only a host that bursts hundreds of requests and stops reading could notice.
- Gaps accepted and recorded: generator-level fixture cases belong to `tests/mcp_contract.rs`; a misbehaving child (no envelope, non-UTF-8, over 16 MiB) and cancel-before-spawn are covered only in the Implementer's unit tests.
- `suite-lock.sh` runs the suite as a background job, so test binaries inherit SIGINT as ignored; it hid a ready-before-trap race in local runs (logged for W6 and batch 13).
- Under `flake-check.sh` CPU load four `tests/mcp.rs` tests time out in their 10 s held-job wait; they fail the same way on a clean earlier checkout and pass in full suites. Logged for W6 as load sensitivity.

## Harness friction

- The conformance writer took about 70 minutes (longest of the run) because the machine was shared with the Implementer's suites.
- Codex planner stalls (four across W4 and W5); its review passes return.
- Carried for batch 13: omp deadline and heading tolerance; stale `glen` binary fallback; `cargo` not on PATH; Implementer commit trailers sometimes say Opus; usage limits idle the run silently; checkpoint commits need a fresh branch after a merge; chained commands must not push after a failed test step; cargo from Python on macOS 27 needs `SDKROOT` unset; `sd` not on PATH, `sed -i` and `cat` blocked by hooks.

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys; the interactive Channels check (AC-505d) is an owner step. Open for the owner: whether glen's routing moves from Codex to omp; the upstream-to-agent-os step; the agent-os port session's questions.

## Next hour

1. W5: my gate and full suite on `f486d13` (quiet machine); live legs; PR, CI, memory, merge, closure.
2. W6 (tech-debt and pre-release audits) planning.
