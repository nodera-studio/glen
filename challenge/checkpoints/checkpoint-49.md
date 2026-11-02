# Checkpoint 49 — 00:06Z → 01:07Z, 2026-11-02 (v0.2.0 run, hour 6)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0 foundation | Merged (PR #35, `35b7522`). |
| v0.2 W1 omp compat (`c1ea2657`) | **In review, draft PR #36**, head `4b0317c`, CI green on every job including the `omp-latest` nightly job. Review round 1 (Claude ‖ Codex) found two must-fix items and one recommended fix; all four fixes are committed and re-verified (gate clean, 1636 tests). Scoped re-review round 2: Claude and Codex approve; the omp pass is still running. |
| W2–W7 | Pending. |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

## Landed on `main` this hour

None.

## Decisions

- **W1 review round 1.**
  - `doctor --probe` had no SIGINT teardown (Claude + Codex, 2 votes): fixed.
  - The probe's late-result handling was untested and a late stage could leak into the snapshot (2 votes): fixed, with the rule extracted and boundary-tested.
  - The surface-test teardown skipped group cleanup (Codex): fixed.
  - The nightly summary mislabelled an unparseable version as a bump candidate (Claude): fixed.
- **omp on large diffs.** The omp pass timed out on the full W1 diff (42 files, 2.6k lines) with no result; it is running on the 7-file fix diff.
- **Deferred as tech debt (recorded in the state log):** the leading-zero and unbounded pre-release handling in the version parser; the probe's spawn ETXTBSY retry.
- **Round 2 findings, to apply in a tiny docs-and-test fix:** the stderr-join timeout branch is untested (Claude LOW + Codex P3: 2 votes) and §A19 should note that SIGTERM and SIGHUP end `doctor --probe` as SIGINT (Claude). Codex's second P3, covering an inherited ignored SIGINT, is a note.

## Harness friction

- Staging and launching the review gate in separate steps let the session idle for 1 h 45 min last hour; one message must do both. Open for batch 13.
- omp reviews of a large diff time out; split the omp pass by file group or raise its deadline. Open for batch 13.
- Still open for batch 13: the omp engine discards a review with a preamble; stale-glen-binary fallback; `cargo` not on PATH; the Codex planner's result needs extracting from the handback call; `## Unreleased` and the "run it as a workflow" wording in the harness docs.

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys. Open for the owner: whether glen's routing moves from Codex to omp, whether each harness batch and W7 get an upstream-to-agent-os step, and the agent-os port session's questions (GPT model id, PR scope).

## Next hour

1. W1: apply the two small round-2 fixes, AC source check, conformance tail, real-omp evidence in a scratch HOME, mark PR #36 ready, merge.
2. Start W2 (the headline) with a dual-engine step plan.
