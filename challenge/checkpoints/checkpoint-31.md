# Checkpoint 31 — hour 31 (15:01Z → 16:01Z, 2026-10-31)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
Checkpoints 25–31 are staged in the main checkout and go into the next wave PR.

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W5, W3b, W6, W7, site fix | Merged. |
| W9 omp engine | **Fix cycle 4 done** (`a93c791`, `1438878`, `37a52bd`).<br>**Scoped re-review:** Claude PASS (0 HIGH, 3 MED); Codex REQUEST CHANGES (1 P1, 5 P2).<br>• Key finding: the 443-only outbound rule blocked omp from reaching the local Mnemosyne MCP, which W9 acceptance needs.<br>**Fix cycle 5 in progress** (`fixes5.md`):<br>• `main` merged in (`4d7dfd5`, `83068f3`);<br>• launcher containment by real path;<br>• loopback-only access to the Mnemosyne port;<br>• deny symlink creation;<br>• the `--thinking` warning points to `glen thinking`. |
| W10 Tech-debt paydown | **Draft PR #23**, Linux CI loop at round 4 (`0c4f18e`).<br>• Steps 18–19 bwrap suite `457205d`.<br>• Review-gate fixes `f3d1eb7`, `fa64a7f`.<br>• CI fixes `42826d9`, `76ddba1`, `042be28`, `0c4f18e`.<br>`ubuntu-latest` went green at `042be28`. `linux-sandbox` is still red (s6). The F8 abort-queue race is under root cause. |
| W8 Audits | **Done via workflow** (105 agents: 16 finders, 14 Claude and 2 Codex, then adversarial verification and synthesis):<br>• 56 raw findings; 2 refuted by the verifiers;<br>• after dedup: **0 HIGH, 14 MED, 30 LOW**.<br>Report: `.claude/reviews/w8-audit-2026-10-31.md`. Fix plan: `.claude/plans/v010-wave-8-audit-fixes.md` (groups G1–G9). |
| W8 Audit fixes | **Fix-wave workflow running:**<br>• 10 parallel group worktrees `fix/w8-g*`, based on W10 tip `042be28`; G9 site copy by the Opus UI implementer;<br>• then integration into `chore/v010-wave-8-audit-fixes`;<br>• then the G8 docs pass;<br>• then a Claude ‖ Codex review and one fix cycle. |
| v0.1.0 release | Pending: W10, then W9, then W8 merge (each merging `main` in), then the release skill. |

## Landed on `main` this hour

None.

## Decisions

- **The owner opted in to workflows**, with at most 20–30 agents at a time. The machine cap is 8 concurrent agents (10 CPUs).
- **W8 audits ran before W9 and W10 merged**, on a read-only snapshot of `main` plus W10 (`42826d9`), to save wall-clock time. The pending W9 harness engine is out of audit scope: it isn't shipped and has its own review gate.
- **W8 fix groups are based on W10's tip**, not `main`. Nearly every audited file is touched by W10. W8 merges `main` in after W10's squash-merge.
- **W9 fix-cycle-5 decisions** (`fixes5.md`): omp may reach the Mnemosyne MCP on its loopback port only, never over the network; symlink creation is denied; the launcher is content-addressed and checked by real path.
- **W10 fix cycle 1:** decisions are in `v010-wave-10-tech-debt.fixes.md`.
- **Release workflow dry run is gated.** The `gh workflow run release.yml` dry run in G7 waits for the owner's go-ahead.

## Harness friction

- **The W9 branch was cut before W3b merged**, so its live-route hint could not name `glen thinking`. Long-lived wave branches drift. Merge `main` in at the start of each fix cycle, not only before the PR.
- **Running the CI loop through a background implementer works, but it is chatty.** Every red head triggers an app event. I mostly confirm the implementer is already on it. A per-PR "owner of CI fixes" marker would let these events skip straight to that agent.
- **The audit verifiers killed only 2 of 56 findings.** Either the finders were precise, or a single REPRODUCE lens is too lenient for LOW items. The 14 MED items each had two lenses. Watch the fix-wave review for false positives.

## Blockers

None. Owner actions outstanding:
- rotate the temporary Cloudflare token and the R2 keys;
- give the go-ahead for the release workflow dry run when W8 gets there.

## Next hour

1. **W10:** get `linux-sandbox` green, then a scoped re-review of `457205d..` (Claude ‖ Codex), the flake-proof N=20 label run, and the merge.
2. **W9:** finish fix cycle 5, then a scoped re-review, the step-11 live smoke (real omp review and debug pass, plus Mnemosyne visibility), then the PR.
3. **W8:** the fix-wave workflow completes; open its PR after W10 merges.
