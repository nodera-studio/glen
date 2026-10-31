# Checkpoint 32 — hour 32 (16:01Z → 17:01Z, 2026-10-31)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
Checkpoints 25–32 are staged in the main checkout and go into the next wave PR.

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W5, W3b, W6, W7, site fix | Merged. **The `main` CI on `7f37ec1` is green again** after a rerun: Lighthouse performance had come in at 0.87, a one-off. The site deploy for #22 ran. |
| W10 Tech-debt paydown | **Draft PR #23** at CI round 5 (`24bee32`):<br>• `linux-sandbox` and `ubuntu-latest` are **green**.<br>• s8: the loopback check now reads the caller's netns.<br>• F8: the test waits for a published request instead of a margin.<br>• TUI deadline test: now driven by a gate.<br>Remaining: macOS `ac363`. Two prep investigations both traced it to a race in the test itself: the worker's 200 ms poll answers the expired request and removes it before the test reads it. The fix (freeze the worker with SIGSTOP while inspecting) is in progress in the W10 CI-fix workflow. |
| W9 omp engine | Fix cycle 5 is in progress. `main` is merged in (`4d7dfd5`, `83068f3`). Next: a scoped re-review, the step-11 live smoke (with Mnemosyne visibility), then the PR. |
| W8 Audit fixes | All 10 code groups are committed (`fix/w8-g1` … `g9`).<br>Integrated into `chore/v010-wave-8-audit-fixes` at `722260f`.<br>The workflow is now on the G8 docs pass, then the Claude ‖ Codex review and fix cycle. |
| Harness batch 10 | **PR #24 opened**, not merged; workflow: 8 parallel groups, then integration, a Claude review (1 HIGH, 9 others), and fixes. It covers:<br>• per-job state authority (including review jobs);<br>• the owner merge waiver;<br>• the secret-paste hook;<br>• probe isolation;<br>• one routing source;<br>• loop rules;<br>• a workflow fan-out guide.<br>**Held:** once #24's routing says "claude", `orchestrator.md`, a W9 file, would still dispatch `impl-coder-codex`. It merges with phase B after W9. |
| v0.1.0 release | Plan at `.claude/plans/v010-release.md`. Gaps:<br>• `Cargo.toml` is already `0.1.0`, so the release PR needs real content (README fixes and a CHANGELOG);<br>• the release workflow has never run on `main`;<br>• `releaseAvailable` flips only after the assets exist, together with the pre-release Playwright tests. |

## Landed on `main` this hour

None. The `main` CI rerun went green and deployed #22.

## Decisions

- **ac363 is a test race, not a product bug.** Two independent investigations (`.claude/research/ac363-race-{product,harness}.md`) agree. The fix is test-only and changes no assertion.
- **Hold PR #24 (batch 10)** until W9 and phase B, so `/implement` routing stays consistent.
- **Release PR content:** since the version is already `0.1.0`, the release PR carries the README fixes from W8 G8 plus a CHANGELOG.
- **Owner:** a project memory about workflow use is not needed, since the run is close to the end. It was deleted.

## Harness friction

- **Two agents started the same ac363 fix in one worktree.** The CI-fix workflow's fixer was editing the test when a separate agent arrived with the prepared diff. The second agent saw the fresh uncommitted edits and stopped cleanly. Fix: one CI owner per PR, which is batch-10 G6's rule, now in #24.
- **The CI-fix workflow's fixer can't be messaged.** New findings, such as the ac363 research, can't reach a running workflow agent. Hand findings over at workflow boundaries, or include the CI log scan in the fixer's own loop (it does).
- **Running four workflows plus two implementers at once** works, but cargo builds compete for 10 CPUs. CI wall-clock, not local work, was the limit this hour.

## Blockers

None for agents. Owner actions outstanding:
- rotate the temporary Cloudflare token and the R2 keys;
- give the go-ahead for the release workflow dry run and for pushing the `v0.1.0` tag.

## Next hour

1. **W10:** the ac363 fix lands and macOS goes green. Then a scoped re-review, the flake-proof N=20 run, and the merge.
2. **W8:** the workflow finishes the docs, review and fixes. After W10 merges, merge `main` in, open the PR, merge.
3. **W9:** finish cycle 5, re-review, live smoke, PR, merge. Then batch-10 phase B on top of #24, and merge both.
4. **Release:** the release PR (README, CHANGELOG), then the owner go-ahead for the dry run and the tag.
