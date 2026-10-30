# Checkpoint 16 — hour 16 (16:01Z → 17:01Z, 2026-10-30)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.

## Wave ledger

| Wave | State |
| --- | --- |
| W0, W1, W2, W3, W6, W7 | Merged (PRs #3, #9, #10, #16, #7, #12). Site live at https://glen.nodera.workers.dev |
| W4 Agent surface | **All six acceptance modes pass:** a, b (one recorded rerun), b2, b3, c1 and c2 (rerun with the fixed wrapper). The summary is `.claude/evidence/w4-acceptance/README.md`. `~/.codex/config.toml` was restored after both c2 runs and is byte-identical to the backup (`cmp`). A 30-min auto-restore watchdog was armed during each run and then stopped. The harness branch `fix/w4-acceptance-harness` has 10 fix commits. A dual review (Claude and Codex) found 14 items, mainly checks that were too loose, and the Codex fix cycle for them is running. After that, the 6 recordings are re-checked under the stricter rules and the PR is opened. |
| W3b Session controls | Codex implemented steps 1–11: WIP `c6cd6f2`, host-green. The review gate is done:<br>• Codex: 2 P1 (the inbox control deadline; omp leaking on a cancel during ephemeral startup) and 2 P2 (a control applied twice when the job ends; omp stderr leaking into `--json`).<br>• Claude: 2 MED (the same duplicate, reproduced; missing contract runners for the new commands) and 8 LOW.<br>**Step 12 live smoke** passed everything except one defect: on a TUI, `compact` reports success when omp's compaction failed. Fix brief `v010-wave-3b-controls.fixes.md` has 6 must-fix items and 6 low; it is dispatched after the conformance writer finishes. |
| W5 `glen watch` | `e3c1037` step 4 (layout and view, a pure TestBackend draw) and `d689b62` step 5 (data-source threads). Steps 6–7 (controls stub, runtime and CLI wiring) are running. Three Codex questions were answered as plain readings of the plan, with no new decisions. |
| W9 | Pending, after W5. |
| W10 Tech-debt paydown | 14 items (board `de08958c`). New: item 14 (headless Claude Code needs the Skill tool pre-allowed; document this for users). Items 7, 10 and 13 gained evidence: three more flaky tests seen, and every acceptance agent hit `--cwd` on `tail`/`send`. |
| W8 Audits + v0.1.0 | Pending, after W10. |

## Landed this hour

Nothing new on `main`.
- **On `fix/w4-acceptance-harness`:** `38fc342`, `20dfd6b`, `1174d39`, `a993c19`, `ce2c232` and `3e05016`, a series of checker and contract fixes from the live runs.
- **On `feat/v010-wave-3b-controls`:** `c6cd6f2` (WIP, steps 1–11).
- **On `feat/v010-wave-5-watch`:** `e3c1037` and `d689b62`.

## Decisions

All are orchestrator decisions, recorded in the briefs and the evidence README.
- The acceptance isolation check accepts exactly one trailing exit-status echo, because the task asks for exit codes and Claude Code's Bash tool hides them on success.
- Usage-rejected calls are not delivery attempts.
- The recording wrapper passes `--help`/`-h` through; the review is now tightening this to exit-0 text only.
- `completions --json` spilling stays, under W0's spill exception, and gets documented (W3b LOW-006).
- c2 was re-run under the owner's standing c2 approval, with the same backup, watchdog and restore procedure.

## Harness friction this hour

- **The acceptance checker loop needed 10 fixes over 6 live runs.** Each run exposed the next false negative. A faster loop would replay the stored recordings against the checker offline after every checker change; that is now the practice. A live re-run is needed only when the wrapper itself changes.
- **Relaxing a check is risky.** The dual review caught one relaxation that also loosened the fixture contract test, and a transcript filter that fails open. Lesson: every relaxed check needs a fixture for both the passing and the failing side, in the same commit.
- **The Implementer wrapper keeps editing test code by hand** during host re-verification (W3b `tests/controls.rs`). This is batch 9 material: diagnose on the host, and send the fix back to Codex.

## Blockers

None. Still waiting on the owner: the `CLOUDFLARE_API_TOKEN` repo secret for CI deploys.

## Next hour

1. Acceptance harness: finish the review fix cycle, re-check the 6 recordings, open the PR, check CI, merge, and close W4 on the board.
2. W3b: run conformance, then fix cycle 1 (6 must-fix items, including the duplicate control and `compact` reporting a failure as success), then a scoped re-review and a smoke re-run of the compact case, then the PR.
3. W5: finish steps 6–12. Once W3b clears its review gate, merge W3b into W5 and switch the controls on.
4. Harness batch 9.
