# Checkpoint 53 — 07:19Z → 08:20Z, 2026-11-02 (v0.2.0 run, hour 9)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0, W1 | Merged (PR #35 `35b7522`, PR #36 `715b1ff`). |
| v0.2 W2 replies without polling (`c5190ea3`) | **Last open item before the PR.** Branch `feat/v020-wave-2-push`, head `758f4d9`; gate clean and 1786 tests pass on my own re-run. Done: build, review (Claude ‖ Codex ‖ omp) and one fix cycle, the AC source check (all consistent), the docs review, and the 60-test blind conformance suite (no contradiction found). The live `claude -p` run is done and an Implementer is correcting the checker's wake classification and writing the §A20 / CHANGELOG / evidence updates. Then the PR. |
| W3–W7 | Pending. |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

## Landed on `main` this window

None.

## Decisions

- **Live `claude -p` result (AC-v2-206).**
  - Claude Code 2.1.316, stream-json, fake omp reply after 100 s: the agent ran `glen wait … --for reply` as a background task and ended its turn, and a first `result` event was emitted. The process stayed alive, and about 100 s later the task's completion notification arrived and the agent resumed and read the reply.
  - The checker's `no_wake` verdict came from comparing the delivery against the first `result` event. I judged that rule wrong, not the transcript; the checker is being corrected (negative cases kept), the recording re-scored, and both classifications recorded in the evidence README.
  - If the corrected score is `woke`: §A20's `claude -p` row becomes `yes`, no skill change, with the limits stated (stream-json only; plain `-p` and interactive untested).
- **Skill permissions in a resumed turn.** Under `dontAsk` with only `Skill(glen)` allowed, the skill's `Bash(glen:*)` permission did not carry into the resumed turn, so a second background wait was denied. That is an artifact of that permission mode, noted in the evidence.
- **W2 conformance findings.** The blind writer found no contradiction with any AC; one doc gap (`FAKE_EXT_*_RELEASE` missing from `tests/fixtures/README.md`) is in the current fix.

## Harness friction

- Still open for batch 13: omp's deadline and heading tolerance; a usage limit stops the loop silently; stale `glen` binary fallback; `cargo` not on PATH; the Codex planner's result needs extracting from the handback call; `## Unreleased` and "run it as a workflow" wording in the harness docs; the conformance writer signs its commit with the wrong model trailer (it used the session's Sonnet line, not the Opus line the brief gave).

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys. Open for the owner: whether glen's routing moves from Codex to omp; whether each harness batch and W7 get an upstream-to-agent-os step; the agent-os port session's questions.

## Next hour

1. W2: finish the checker correction and §A20 update, my deliberate gate and test re-run, push, PR, CI, merge, close the wave.
2. Start W3 (the hidden ACP spike) with a dual-engine step plan.
