# Checkpoint 18 — hour 18 (18:01Z → 19:01Z, 2026-10-30)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.

## Wave ledger

| Wave | State |
| --- | --- |
| W0, W1, W2, W3, W4, W6, W7 | Merged: PRs #3, #9, #10, #16, #15 + #18, #7, #12. Site live at https://glen.nodera.workers.dev |
| W3b Session controls | Fix cycle 1 is in progress. Group (a) `906ff57`: a control applied twice at job end, the inbox deadline, `job_ended`, and lock handling while a job finishes. Group (b) `d3a232d`: on startup cancel, omp is reaped before the lock is released, and child stderr is captured. Group (c) `420451d`: compact failing without throwing is now detected, and the control socket closes reliably under load (D1). Still to do: group (d), contract runners and docs; group (e), rendering and test tightening. |
| W5 `glen watch` | **Steps 1–11 implemented**: `9caf272` snapshots and purity guard, `36f29d6` real-pty tests, `8bddbdb` docs. The review gate is running. Codex is done, with 7 P2 findings: a second signal during shutdown, follow failures hidden behind cached lines, rewrites of the same size and growth rewrites, v1 rewrite IDs, timezone I/O in the pure path, grapheme widths, and a weakened `GLEN_HOME` assertion. The Claude visual review and the blind conformance writer are still running. |
| W9 | Pending, after W5. |
| W10 Tech-debt paydown | 14 items, board `de08958c`. |
| W8 Audits + v0.1.0 | Pending, after W10. |

## Landed on `main` this hour

- `70d1460`, PR #19: **harness batch 9** (memory `3eaebfec`).
  - Codex writes all code.
  - WIP commits after each group of steps.
  - A live smoke run for harnesses that drive a TUI.
  - Worktree-removal scripts that signal only brokers.
  - Plan conditionals resolved when the plan is written.
  - It took three review rounds. The first was a BLOCK, after reproducing kills of the wrong process. The scripts were then cut back to brokers only. 67 dummy-process cases pass.

## Decisions

- **Process-killing helpers signal only brokers.** Job and task cancellation is done by the orchestrator through the companion's `cancel`, and the script refuses while anything is live (memory `3eaebfec`).
- **W5 pty harness, split by platform.** macOS uses a controlling TTY for signal and escape tests, plus a non-controlling pty to check termios. Every run must assert raw mode *during* the TUI and the restored state *after*, so the test cannot pass vacuously.
- **W5 shared-surface edits stay narrow.** Its skill change is one guarded line, and `json.md` gets only its own `watch` section. W3b owns the rest.

## Harness friction this hour

- **First real use of `remove-worktree.sh` was a false refusal.** A stale entry in the companion's `state.json` index said `running`, while the per-job file said `completed` with `pid:null`. It failed safe, and I removed the worktree by hand. Batch 10 will make the per-job file the authority.
- **Each review round of the process-killing scripts found a new way to signal the wrong process.** The fix was to narrow the scope, not to keep patching. This is recorded as a lesson in `3eaebfec`.

## Blockers

None. Owner action still pending: the `CLOUDFLARE_API_TOKEN` repo secret for CI deploys.

## Next hour

1. **W3b:** fix groups (d) and (e), then a scoped Claude ‖ Codex re-review. Then 10 consecutive green conformance `node_` runs, a re-run of the live compact smoke, and the PR.
2. **W5:** fold in the Claude visual review, the conformance results and the 7 Codex P2 findings, then fix cycle 1. After the W3b gate, merge W3b, enable the controls, and run the Step 12 manual check.
3. **Batch 10 (small):** make the per-job file authoritative in `codex-procs.sh`.
