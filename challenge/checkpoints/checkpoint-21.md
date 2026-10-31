# Checkpoint 21 — hour 21 (21:01Z → 22:01Z, 2026-10-30)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
This file is staged in the main checkout together with checkpoint 20, and both go into the next wave PR.

## Wave ledger

| Wave | State |
| --- | --- |
| W0, W1, W2, W3, **W3b**, W4, W6, W7 | Merged. **W3b: PR #20 → `44c9d5b`** (memory `4e50511b`), board done. |
| W5 `glen watch` | Fix cycle 1 is done. Group e: `6e4401c`. W3b merged into W5: `51ef30f`. The W3b blind test `ac351` was migrated so `watch` is visible. Controls are being wired to the real `src/ctl`. Two W5 blind `ac516` tests are being migrated to "controls available", and they assert real op frames. Items 15 and 21 are still to do: the strict completions assertions and the control snapshots. |
| W9 omp engine | Implemented offline: `2b8b542`…`0556986`, 32 files. **The review gate blocked it.**<br>• Claude: 3 HIGH, 4 MED, 9 LOW.<br>• Codex: 1 P1, 3 P2.<br>Findings:<br>• read containment can be bypassed through a `.env` and through `@`-imports or symlinks in context files;<br>• `modelRoles.default` is not a real omp key, so setup aborts;<br>• the size check counts `target/`, so every review is skipped;<br>• `omp config path` can write the default config;<br>• the guard hook has bypasses and false positives;<br>• dispatch is untested.<br>Fix cycle 1 is running: 17 items, in `v010-wave-9-harness-omp.fixes.md`.<br>**The live smoke still needs the owner to log in to Cursor.** |
| W10 Tech-debt paydown | 14 items. Item 1 is closed by W5 (`220d09c`). |
| W8 Audits + v0.1.0 | Pending, after W10. |

## Landed on `main` this hour

- `44c9d5b`, PR #20: **W3b session controls.** Linux CI first hit `MAX_ARG_STRLEN` in a blind argv test. The fix keeps the test unchanged on macOS and adds a unit test of the same boundary. The worktree was removed with `remove-worktree.sh`, its first successful real run.

## Decisions

- **Migrating blind suites across waves.** When a later wave's plan changes an earlier wave's behaviour, the earlier wave's blind test is migrated at merge time, with a comment citing the plan. Every other assertion stays unchanged. Applied twice:
  - W3b `ac351`: `watch` is now visible.
  - W5 `ac516`: controls are now available. The test asserts real dispatch. The old "not in this build" check is kept under an off-switch if the build allows one.
- **W9:** the stray probe profile was moved out, not deleted. Only `glen-harness` is allowed.

## Harness friction this hour

- **The W9 Implementer's probe created a second omp profile** (`glen-harness-probe`), which owner decision 2 doesn't allow. The orchestrator moved it to the scratchpad. Batch-10 rule: probes use a temporary `HOME`/`PI_CONFIG_DIR` or the approved profile only.
- **The W9 review found a whole class of read-containment issues.** omp's context loaders (dotenv, `@`-imports, symlinked context files) bypass its tool approval layer. Lesson: sandboxing an agent by tool policy alone misses discovery-time I/O.
- **Each parallel wave's blind suite pins the other wave's pre-merge state.** That cost two decision round-trips at merge time. The shared-seam procedure could name these expected migrations up front.

## Blockers

- **W9 live smoke:** waiting on the owner to run `omp --profile glen-harness login cursor`.
- **CI deploys:** the owner still needs to add the `CLOUDFLARE_API_TOKEN` repo secret.

## Next hour

1. **W5:** finish wiring the controls and items 15/21, run the Step 12 manual run against scratch omp, do a scoped re-review, then open the PR, wait for CI, and merge.
2. **W9:** fix cycle 1, then a scoped re-review. Live smoke once the owner logs in. Otherwise the PR waits on the live evidence; the board acceptance requires a real omp pass.
3. **W10:** plan in parallel, now that W3b is merged and W5 will soon be.
