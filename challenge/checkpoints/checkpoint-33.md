# Checkpoint 33 — hour 33 (17:01Z → 18:01Z, 2026-10-31)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
Checkpoints 25–33 are staged in the main checkout and go into the next wave PR.

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W5, W3b, W6, W7, site fix | Merged. `main` is at `7f37ec1` and its CI is green. |
| W10 Tech-debt paydown (PR #23, draft) | CI round 5 went **fully green** at `8980d6f`:<br>• **s8 was a real product bug:** the loopback check read the host's `/sys`; it now reads `/proc/self/net/dev`.<br>• F8, ac363 and the TUI test were test races, now event-driven.<br>**Blind conformance** `60daf89`: 38 tests, 34 pass. The 4 failures are 3 W10 docs gaps plus AC-1027 (deferred to batch-10 phase B).<br>**Re-review:** Claude BLOCK (2 HIGH: the narrowed netns rule is wrong for Codex's real `--unshare-pid` sandbox and for kernels with tunnel devices); Codex REQUEST CHANGES (4 new P2).<br>**Flake-proof run 1:** F9 in `jobs::lock::holder_tests`.<br>**Fix cycle 2 workflow running:** 5 parallel groups, 4 of them committed so far. |
| W9 omp engine | **Fix cycle 5 done** (`068c4e7`, `78abe2c`, `c9a634a`, plus the merge of `main`). **Mnemosyne reached from the sandboxed omp, verified live.**<br>**Acceptance workflow running:** Claude ‖ Codex re-review in parallel with the full Step 11 live acceptance, then fixes, then the PR. |
| W8 Audit fixes | **Fix wave done** at `40c8830` on `chore/v010-wave-8-audit-fixes`: 10 groups, then docs (G8), then review.<br>• Claude PASS (2 MED, 7 LOW).<br>• Codex REQUEST CHANGES (3 P2).<br>• All MED and P2 items fixed in `40c8830`, including the `GLEN_HOME` trust check on read paths.<br>Waits for W10 to merge, then merges `main` in, then the PR. |
| Harness batch 10 | PR #24, CI green, **held** for W9 and phase B. |
| v0.1.0 release | **Owner approved the dry run and the tag push.** Dry run 1 on `main` passed (run 41223152464: all 4 targets, `verify-install` on macOS and Linux). |

## Landed on `main` this hour

None.

## Decisions

- **Owner approval (chat):** the release workflow dry run and pushing the annotated `v0.1.0` tag on the release PR's squash commit.
- **AC-1028 reverts to the blanket rule.** On Linux, any Codex marker means the lease is hidden and every RPC resume is refused; the live route still works. Recorded in `fixes2.md` decision 1, superseding `fixes.md` decision 1.
- **Revoked:** `fixes.md` decision 8 and `fixes2.md` decision 10, which said to mark `--cwd` orphans as read. AC-1011 says they stay unread, and the blind conformance tests pin that.
- **AC-1027 test** is `#[ignore]` in W10 until batch-10 phase B lands Step 21.

## Harness friction

- **Two orchestrator decisions failed review.** One was the "narrowed" netns rule; the other misread AC-1011. In both cases the blind conformance tests or the dual review caught the mistake before merge. Lesson: when the orchestrator writes a fix-cycle decision, it should quote the AC it applies, verbatim.
- **Agents inside a running workflow can't be corrected mid-flight.** Group B is coding a revoked decision. The fix-up has to happen after the workflow, which costs one extra round. Batch-11 candidate: workflows re-read the brief file before committing, so a revocation can still land.
- **A watch TUI test (`ac514`) fails on every run on this Mac, including on `main`,** yet CI is green. An investigator is on it, together with the `live.rs` F10 candidates.

## Blockers

None. The owner should rotate the temporary Cloudflare token and the R2 keys.

## Next hour

1. **W10:** fix cycle 2 integrates, gets pushed and runs CI, then the dual re-review. After that, revert the orphan marking, add the doc rows, ignore the ac1027 test, rerun the flake-proof label, and merge.
2. **W9:** acceptance and re-review, then fixes, then the PR, then merge after W10.
3. **W8:** merge `main` in after W10, then the PR and the merge.
4. **Release PR:** the README and CHANGELOG, then dry run 2, then the tag.
