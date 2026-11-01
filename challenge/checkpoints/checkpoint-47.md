# Checkpoint 47 — 21:18Z → 22:18Z, 2026-11-01 (v0.2.0 run, hour 4)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done (`.claude/plans/v020.architecture.md`). |
| v0.2 W0 foundation | Merged (PR #35, `35b7522`). |
| v0.2 W1 omp compat (`c1ea2657`) | **Implemented, unreviewed.** Branch `feat/v020-wave-1-omp-compat`, head `319e519` (6 commits: steps 1–14 by the Implementer). The Implementer's report is still to come; next are my deliberate gate and test re-run, the Claude ‖ Codex ‖ omp review, the AC check, the conformance tail, the real-omp evidence run, and the PR. |
| W2–W7 | Pending. |
| agent-os port (PR #2) | Open, not merged. |

## Landed on `main` this hour

None.

## Decisions

- **W1 plan:** dual planners (Claude, Codex) and a blind judge picked the Claude plan as the spine, with 3 Codex grafts: two extra source facts, real omp always run in a scratch cwd, and the reference-tree commit recorded in the evidence. The plan of record is `.claude/plans/v020-wave-1-omp-compat.md`.
- **ACs the plan amended, all judged justified:**
  - AC-v2-102: unparseable `--version` output is a warning.
  - AC-v2-103: 30 s budget plus bounded teardown; "required capability" is `host_tools` only.
  - AC-v2-104: narrowed to jobs (foreground controls have no status file).
  - AC-v2-105/106: real omp needs a loopback stub-provider `models.yml` to reach `ready`.
- **Owner-visible:** AC-v2-104's narrowing. A protocol field in control results would be a `docs/json.md` contract change, outside W1.

## Harness friction

- The omp engine discards a review with a preamble; it fell back to a stale `target/release/glen`; `cargo` is not on the default PATH after the account switch. All three are open for batch 13 (already in the decision log).
- The plan orchestrator needed a manual transcript extraction to save the Codex planner's result (its final message arrives as a handback call, not text). Open for batch 13: save the artifact from the handback message.

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys. Open for the owner: whether glen's routing moves from Codex to omp, and whether each harness batch and W7 get an upstream-to-agent-os step.

## Next hour

1. W1: gate and tests, review (Claude ‖ Codex ‖ omp), AC source check, conformance tail, real-omp evidence in a scratch HOME, PR; check the first `omp-latest` run on the PR.
2. Merge W1; start W2 (the headline) with a dual-engine step plan.
