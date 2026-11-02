# Checkpoint 55 — 09:20Z → 10:21Z, 2026-11-02 (v0.2.0 run, hour 11)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0, W1 | Merged (PR #35 `35b7522`, PR #36 `715b1ff`). |
| v0.2 W2 replies without polling (the headline) | **Merged** as PR #37 → `0157285`; memory `4f05272f`; board done + `shipped:main`. |
| v0.2 W3 omp acp spike (`1918d79f`) | **Implementing** on `feat/v020-wave-3-acp-spike`: plan judged (Claude spine, 11 steps, judge edits G1–G3 and J1–J7); the Implementer has committed steps 1–3 (`e77515f`). Mine to run: Step 7 (the live E1–E3 experiments, 2 real model turns), Step 9 (blind conformance), Step 11 (ADR memory). |
| W4–W7 | Pending. |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

## Landed on `main` this window

- PR #37 → `0157285`: W2. Review: Claude ‖ Codex ‖ omp; a macOS-only teardown race (EPERM from a zombie-only process group) in W1's code was found by CI and fixed at the root in the same PR; 1789 tests pass in two consecutive full runs; the live `claude -p` run showed the completion notification resumes a headless run.

## Decisions

- **W3 plan.** Dual planners and a blind judge chose the Claude plan, with three Codex grafts. The judge corrected seven things in it: the SIGINT exit bound is 11 s; the lease-holder fake needs stdin held open; the scratch path is canonicalized for macOS; the cleanup script's 30-minute default is overridden; the E1 guard check runs against the fake, not real omp; omp's mapping of a provider error to `end_turn` means E2 compares transcripts; scratch cleanup uses `--min-age 0`.
- **Experiments policy.** E1 sends no prompt; E3 and E2 are the only real model turns (2 in total, grok-4.7 on the owner's Cursor usage), scratch sessions only, `--profile glen-harness`, no `agent.db`, no reruns. A provider refusal records AC-v2-304 as blocked and the verdict as inconclusive, never GO. The go/no-go rule is pre-registered in the plan.
- **Protocol facts settled from the omp source.** `stopReason` is on the prompt response, not in an update; `session/load` replays history as updates before it answers; `session/fork` exists in this pin (out of scope); `initialize` ignores the client's version and returns 1, so the check is on the response.

## Harness friction

- Still open for batch 13: omp's deadline and heading tolerance; a usage limit stops the loop silently; stale `glen` binary fallback; `cargo` not on PATH; the Codex planner's result needs extracting from the handback call; `## Unreleased` and "run it as a workflow" wording in the harness docs; the conformance writer signs commits with the session's model trailer, not the brief's; chained commands must not push after a failed test step; macOS group-signalling EPERM is a recurring pitfall (helpers now tolerate it).

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys. Open for the owner: whether glen's routing moves from Codex to omp; whether each harness batch and W7 get an upstream-to-agent-os step; the agent-os port session's questions.

## Next hour

1. W3: the Implementer finishes steps 4–6, 8 and 10; then my gate and suite re-runs, the three-engine review, the AC check, docs, the blind conformance writer, the live experiments E1–E3, the ADR memory, PR, CI, merge.
2. Start W4 (the opt-in warm worker) with a dual-engine step plan.
