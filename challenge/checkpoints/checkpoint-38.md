# Checkpoint 38 — hour 38 (22:01Z → 23:01Z, 2026-10-31)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
Checkpoints 25–37 are on `main` (they shipped with W9). This checkpoint goes into the next wave PR.

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W7, W3b, site fix | Merged. |
| W10 Tech-debt paydown | **Merged** as PR #23 → `7c334fa`. Memory `56edaa33`; board done with `shipped:main`. |
| W9 omp engine | **Merged** as PR #25 → `8e965cb`, after a clean merge of `main` plus checkpoints 25–37 (`51d5e0d`). CI green on every job: 1519 tests, plus 753 and 254 smoke-test assertions. Memory `b1f3fbd9`; board `aec92571` done with `shipped:main`; re-indexed; worktree removed. |
| W8 Audit fixes (PR #26) | **Opened** and taken to green at `8c2956d`. The W10 merge was redone against the right base (`042be28`), cutting 34 conflicts to 13. The CI round-1 fixes are in **Decisions**. Workflow `wr04qzue9` is now merging `main` after W9 into it. GitHub reports a conflict until that push. |
| Harness batch 10 (PR #24) | Workflow `wr04qzue9` is building phase B: merge `main`, groups B1–B3 in their own worktrees, integrate, then a Claude ‖ Codex review, fixes and CI. |
| v0.1.0 release | Approved. The owner opted in to a `CHANGELOG.md` as the release PR's content; a draft is in the session scratchpad. |

## Landed on `main` this hour

- PR #23 (W10) → `7c334fa`.
- PR #25 (W9) → `8e965cb`.

## Decisions

- **Release PR content:** a `CHANGELOG.md`, which the owner chose in chat. `Cargo.toml` is already `0.1.0`.
- **W8 FAQ e2e test migration:** it now asserts W10's blanket Linux hidden-lease wording, with a comment citing W10 fixes2 decision 1. The narrowed network-namespace wording it asserted before was reverted in W10. The copy was not reverted; the test follows the code.
- **MSRV fix:** `use super::{self as rpc, …}` became `use crate::rpc::{self, …}`. Rust 1.89 rejects the old form, and W8's new `msrv` job exposed it.
- **W9 merge** recorded in memory `b1f3fbd9`.

## Harness friction

- **A squash merge loses the merge base.** Merging `main` into a branch cut from a pre-squash wave commit (W8 from W10's `042be28`) picked an old base and produced 34 conflicts. The fix was `git merge-tree --merge-base=<cut commit>`. Batch-11 candidate: document this in the loop procedures.
- **The autofix conflict event fired while a workflow agent was mid-merge on that branch.** The orchestrator left the branch to the agent, because a second merge would have collided with its in-progress one.
- **`remove-worktree.sh` discarded 3 uncommitted paths** in the W9 worktree, probably git-ignored evidence. Candidate: copy `.claude/evidence/` to the main checkout before removal.
- **The main checkout's untracked checkpoints blocked `git pull --ff-only`** once the same files landed on `main`. They were byte-identical and were moved to the scratchpad.
- **`memory_store` rejects short ids in `relatedIds`;** it needs full UUIDs.

## Blockers

None. The owner should:
- rotate the temporary Cloudflare token and the R2 keys;
- delete the scratch folders `$TMPDIR/glen-flake-*` and `$TMPDIR/glen-w9-accept7`.

## Next hour

1. **W8:** the merge of `main` lands, CI goes green, then the merge memory, squash merge, board update, re-index and site redeploy (credits).
2. **Harness batch 10:** phase B is reviewed and green on PR #24, then the memory and the merge.
3. **Release PR:** `CHANGELOG.md`, checked against `--help` on `main`, plus this checkpoint. Then dry run 2, then merge, then the annotated `v0.1.0` tag, then verify the 9 assets.
