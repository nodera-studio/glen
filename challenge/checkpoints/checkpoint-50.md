# Checkpoint 50 — 01:07Z → 04:52Z, 2026-11-02 (v0.2.0 run, overdue by 164 min)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`. The run stopped at about 02:00Z when the account hit its session usage limit; it resumed at 04:51Z after the owner's reset.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0 foundation | Merged (PR #35, `35b7522`). |
| v0.2 W1 omp compat (`c1ea2657`) | **Draft PR #36, one defect open.** Head `5d8c6e9` (conformance suite) on the branch; CI was green on `4b0317c`, so the later commits need a CI run. Review rounds 1 and 2 are done, the AC source check is consistent, the docs update found nothing to change, the real-omp evidence is collected, and the conformance suite is written: 55 of 56 pass. The one failure is a real defect: a job that fails at host-tool registration never records `rpc_protocol` (AC-v2-104). A fix is half-done and uncommitted in `src/rpc/worker.rs` and `tests/omp_compat.rs` (the Implementer was cut off by the usage limit). |
| W2–W7 | Pending. |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

## Landed on `main` this window

None.

## Decisions

- **Conformance finding.** The blind writer's one failing test is a real AC violation, not a test problem: the plan says `rpc_protocol` is written "right after the omp client starts", but the worker writes it after `start()` returns, which includes the registration that failed. The fix goes to an Implementer with a unit test in a non-conformance file; the conformance file stays as written.
- **Evidence.** The real-omp 18.14.0 surface contract passes (protocol 2); the doctor probe is `ok` with the stub provider and `warn`/all-`unknown` on a bare omp, both exit 0 with no `GLEN_HOME`. Details in `.claude/evidence/v020-wave-1-omp-compat/README.md`.
- **Usage limit.** Two Implementers were cut off by 429s (the first one during the W1 work at the weekly limit, the second during the fix just now). Both left their work on disk; the interrupted one is resumed, not redone.

## Harness friction

- The run stops dead at an account usage limit, with several hourly prompts queued behind it, which is why this checkpoint is 164 minutes late. Open for batch 13: after a 429, the loop should write a `blocked: usage limit` note in the state file instead of leaving the checkpoint overdue.
- Still open for batch 13: stage and launch the review gate in one message; the omp pass times out at 15 minutes even on small diffs (raise `--deadline`); the omp engine's preamble handling; stale `glen` binary fallback; `cargo` not on PATH; the Codex planner extraction; `## Unreleased` and "run it as a workflow" wording.

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys. Open for the owner: whether glen routing moves from Codex to omp; whether each harness batch and W7 get an upstream-to-agent-os step; the agent-os port session's questions (GPT model id, PR scope).

## Next hour

1. W1: finish the `rpc_protocol` fix, rerun the gate and tests, push, CI, mark PR #36 ready, merge, close the wave.
2. Start W2 (the headline) with a dual-engine step plan.
