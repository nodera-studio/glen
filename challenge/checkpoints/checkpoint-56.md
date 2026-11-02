# Checkpoint 56 — 10:21Z → 11:22Z, 2026-11-02 (v0.2.0 run, hour 12)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0, W1, W2 | Merged (PR #35 `35b7522`, PR #36 `715b1ff`, PR #37 `0157285`). |
| v0.2 W3 omp acp spike (`1918d79f`) | **In the review-fix cycle** on `feat/v020-wave-3-acp-spike` (head `8612f82` + checkpoints). Built: the hidden `__acp` command, the synchronous client, the fake `acp` peer, 18 integration tests and 37 unit tests; gate clean and 1844 tests pass on my own re-run. Review round 1 (Claude ‖ Codex ‖ omp) found no critical or high issue; an Implementer is working through the taken findings. Still mine: the live experiments E1–E3 (2 real model turns), the AC check, the docs update, the blind conformance writer, the ADR memory, the PR. |
| W4–W7 | Pending. |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

## Landed on `main` this window

None.

## Decisions

- **W3 review round 1 — what is being fixed.**
  - Two-engine must-fix: `Client::prompt` writes the prompt even when stop or the deadline is already set; unbounded memory in the reader's frame queue, the update-kind keys, and event strings.
  - Single-engine, taken: a permission request after Ctrl-C but before the cancel tick answered under `allow`; `cancel.sent` meaning enqueued, not written; the trailing drain swallowing a malformed frame; the accumulator not frozen during `session/close`; a sleep-ordered test; a `json!`/`Path` panic on non-UTF-8; a relative stored cwd sent to `session/new` (found by omp).
  - Not changed: cancel answered `end_turn` still reporting `canceled`, and the 100 ms teardown after a deadline cancel (by the plan's rule J1).
- **omp as a reviewer.** Its W3 pass ran 18 minutes at a 30-minute deadline and returned a real review with three findings, two of them duplicating other engines' findings. Status was `failed: malformed output` (a text preamble), so it has no vote; its findings were checked against the code and taken where they stood up.
- **Live experiments** wait until the code is final and no omp review is running (same profile and Cursor usage). Plan-pinned rules: E1 sends no prompt; two real turns in total; a provider refusal records AC-v2-304 as blocked, never GO.

## Harness friction

- Still open for batch 13: omp's deadline and heading tolerance (it needs ~18 minutes for a diff this size and then fails to parse its own output); a usage limit stops the loop silently; stale `glen` binary fallback; `cargo` not on PATH; the Codex planner's result needs extracting from the handback call; `## Unreleased` and "run it as a workflow" wording in the harness docs; the conformance writer signs commits with the session's model trailer; chained commands must not push after a failed test step; running cargo from Python on macOS 27 needs `SDKROOT` unset.

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys. Open for the owner: whether glen's routing moves from Codex to omp; whether each harness batch and W7 get an upstream-to-agent-os step; the agent-os port session's questions.

## Next hour

1. W3: the fix cycle lands, my gate and suite re-runs, the live experiments E1–E3, the AC check and docs, the blind conformance writer, the ADR, PR, CI, merge.
2. Start W4 (the opt-in warm worker) with a dual-engine step plan.
