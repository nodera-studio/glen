# Checkpoint 19 — hour 19 (19:01Z → 20:01Z, 2026-10-30)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.

## Wave ledger

| Wave | State |
| --- | --- |
| W0, W1, W2, W3, W4, W6, W7 | Merged. The site is live at https://glen.nodera.workers.dev. |
| W3b Session controls | **Fix cycle 1 is done.** The last two commits are `d111d0e` (contract docs and runners, plus a fixed teardown-ordering flake) and `5e0d629` (models footer, abort wording, `completions` exempt from spill). All 12 items are verified on the host: `dup2.sh` shows a single application, and 10 consecutive green runs of `controls::node_` confirm D1 is fixed.<br>**Live compact re-run passes** (`evidence/v010-wave-3b-controls/compact-rerun/`): a too-small session gives exit 7, a large one compacts from 39k to 14k tokens, and an abort mid-compaction gives exit 7.<br>**Scoped re-review:** Claude says APPROVE_WITH_NOTES (2 MED, 6 LOW). Codex confirmed 3 of 4 items closed and raised 2 new P2s.<br>**Fix cycle 2 is running** (C2-1…C2-9): no retry after a mutation has been submitted (`outcome_unknown`), SIGINT during preflight, startup stderr kept, terminate ordering. |
| W5 `glen watch` | **The blind conformance suite (`41a6b86`) passes 75 of 91.** The 16 failures come from 9 defects, D1–D9. The fix brief merges Codex's 7 P2s, Claude's visual review (2 HIGH, 5 MED, 13 LOW) and D1–D9: 15 must-fix items and 10 low ones. Group (a), `e9c5503`, is done: a second signal during the quit wait, exit 1 on a dead terminal, and the pty scenarios. Group (b), text and hints, is running. |
| W9 | Pending, after W5. |
| W10 Tech-debt paydown | 14 items. Item 1 (`tail -f` re-emitting on a v1 rewrite) will be closed by W5 fix item 12, which touches the shared follow code. |
| W8 Audits + v0.1.0 | Pending, after W10. |

## Landed on `main` this hour

None. Two waves are in fix cycles.

## Decisions

- **`completions` output is exempt from spilling.** The script itself is the payload. W3b implements this because it owns the shared output surface, and the strict test assertions are restored. W5 picks it up when it merges W3b.
- **A control mutation that was already submitted is never retried.** A dropped connection after submission returns `control_failed`/`outcome_unknown`. This supersedes the cycle-1 trade-off.
- **TUI compact errors stay generic.** On the TUI route, glen reports "compaction did not run", because omp's TUI discards the outcome. The RPC route carries omp's own message. C2-6 tries `onError` first.
- **W5 conformance readings R1–R12 are accepted**, including that AC-521 requires one overlay listing every binding.

## Harness friction this hour

- Writing the fix brief from a stale shell cwd failed the first time: a relative path ran from a removed worktree. Use absolute paths in orchestrator shell steps.
- The blind conformance writers proved their worth again. W5's suite found two defects neither review caught: `--color always` doesn't override `NO_COLOR`, and a dead terminal exits 101 instead of 1.

## Blockers

None. Owner action still pending: the `CLOUDFLARE_API_TOKEN` repo secret for CI deploys.

## Next hour

1. **W3b:** finish fix cycle 2, run one quick Codex re-check, then open the PR, wait for CI, record the memory, merge, and close the board item.
2. **W5:** fix groups (b) through (e). Then merge `main` (with W3b) into W5, enable the controls, restore the strict completions assertions, and run Step 12 by hand against scratch omp. Then the scoped re-review and the PR.
3. **W9:** explore and plan, in parallel once W3b has merged.
