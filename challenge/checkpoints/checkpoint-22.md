# Checkpoint 22 — 22:01Z (2026-10-30) → 07:10Z (2026-10-31)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
Checkpoints 20–22 are staged in the main checkout and go into the next wave PR.

**Gap:** about 9 hours passed between checkpoint 21 and this one with no checkpoint prompt. The session was idle, waiting on background agents, which reported during the gap. No work was lost.

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W4, W3b, W6, W7 | Merged. |
| W5 `glen watch` | **Fix cycle 1, the W3b merge and the controls are done** (`39f3183`). On the host: watch:: 91/91 three times, controls:: 69/69, tests/watch 23/23, and the full suite green with 4 threads.<br>**Step 12 live run against real omp 18.14.0: all items passed**:<br>• send, steer and talk-back;<br>• refusal shown inline;<br>• 60×20 and 80×24 layouts;<br>• NO_COLOR and light theme;<br>• `m` and `T`;<br>• `C`, both too-small and real compaction (26k → 11k);<br>• `^x` aborted in 0.32 s;<br>• SIGTERM restores the terminal.<br>Evidence: `.claude/evidence/v010-wave-5-watch/`.<br>Re-reviews: Claude REVIEW_REQUIRED (1 MED, 10 LOW); Codex 4 P2.<br>**Fix cycle 2 is running** (`v010-wave-5-watch.fixes2.md`, 10 must-fix + 9 low):<br>• rapid double signal;<br>• full-prefix rewrite hash;<br>• rebuild the v1 occurrence counts;<br>• incremental v1 appends;<br>• header refresh;<br>• `· ·` separator;<br>• restore the item-22 stand-ins and the ac516 "mTC" assertion;<br>• preselect the current choice in pickers. |
| W9 omp engine | **Fix cycle 1 is done** (`44086be`…`f8d3998`, 17 items):<br>• `.env` quarantine;<br>• refusal of symlink escapes;<br>• `@`-imports neutralised;<br>• post-copy size check;<br>• `modelRoles` record merge;<br>• the default dir computed without starting omp;<br>• the guard hook rewritten as a Python shell-token parser that fails closed;<br>• dispatch smoke coverage (670 + 108 assertions).<br>Scoped re-reviews by Claude and Codex are running. The live smoke still needs the owner's Cursor login. |
| W10 Tech-debt paydown | Explored (research `d31bea2d`). **The owner decided items 4, 5, 8, 9 and 11** (memory `569ec6f9`). Item 3 was already fixed. New item 15 is a controls.rs deadline flake. The flakes in items 7, 10 and 15 share one root cause: sub-second wall-clock margins. Full-suite flake rate is 2 in 5. |
| W8 Audits + v0.1.0 | Pending, after W10. |

## Landed on `main` this period

None. Three waves are in fix or review cycles.

## Decisions

- **Owner, W10** (memory `569ec6f9`):
  - EPIPE on stdout exits 0 silently, and AC-029 is amended.
  - An Ubuntu CI job simulates the Linux sandbox with bwrap/unshare.
  - Item 9 is accepted as a documented gap.
  - install-extension uses an atomic swap: RENAME_EXCHANGE or RENAME_SWAP.
  - The harness verifies each AC against the source before conformance.
- **Orchestrator:** the W5 follow-rewrite check hashes the full prefix whenever size or mtime change, with a 4 KiB fast path. Codex's two findings on W3b assertion changes inherited from main (`rpc.rs:1620`, `ext_node.rs:494`) were already accepted by W3b's reviewers as plan-driven spec changes, so no action.

## Harness friction

- **A long idle gap had no heartbeat.** The hourly cron is session-only and fires only while the session is active. During a 9 h wait on agents, no checkpoint was written. Candidate for batch 10: a ScheduleWakeup fallback during long waits.
- **The W5 Implementer's report wrongly said item 22 was done.** The reviewer caught it. Report claims need the reviewer's verification, which the gate already provides.

## Blockers

- **W9 live smoke:** the owner must run `omp --profile glen-harness login cursor`.
- **CI deploys:** the owner still needs to add the `CLOUDFLARE_API_TOKEN` repo secret.

## Next hour

1. **W5:** fix cycle 2, then a quick re-check, then the PR, CI and merge.
2. **W9:** fold in the re-review results. Then either do the live smoke after login or open the PR with the live evidence pending. The board's acceptance criteria require a live pass.
3. **W10:** plan and implement, starting with the flake root-cause fix.
