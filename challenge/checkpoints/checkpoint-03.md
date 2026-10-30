# Checkpoint 3 — hour 3 (03:01Z → 04:01Z, 2026-10-30)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.

## Wave ledger

| Wave | State |
| --- | --- |
| W0 Foundation | **Merged** as PR #3 (`fb4e457`) |
| W1 Disk reader (`ls`, `show`, `tail -f`) | Implemented as `fb66f38`: 240 tests, live-checked against a scratch omp session. Review gate: Claude found 3 medium and 11 low; Codex found 7 P2 (most overlap Claude's). **Fix cycle 1** (21 items) is running in worktree `w1-fix`, with the conformance writer running on the wave branch at the same time. |
| W2 RPC driver (`send`, `wait`, `jobs`) | Plan **judged blind**: base B plus 6 additions from A; revealed after the verdict, A = Codex and B = Claude. Network pre-flight committed (`e83dbbb`). **Implementer running** in worktree `w2-rpc`, based on W1 and to be rebased after W1 merges. |
| W3 Live route + reply channel | Pending |
| W3b Session controls (abort, model/provider, thinking) | Pending. Added to scope this hour (original intent). W2's worker keeps a control seam for it. |
| W4 Agent surface + acceptance | Pending |
| W5 `glen watch` | Pending |
| W9 omp in agent-os | Pending (after W5) |
| W6 Site | Implemented (`9b204c2`). The visual check found a terminal-demo defect that both code reviewers missed. **Review-fix pass running** (the demo, contrast, CI artifacts, 9 low items). |
| W7 Deploy + release workflow | Pending |
| W8 Audits + v0.1.0 | Pending |

## Landed on `main` this hour

- `26c0f2e`, PR #5 — harness batch 3:
  - blind plan judge (coin-flip A/B, mapping kept private);
  - planner check that each test matches its step;
  - visual verification gate in `/implement`.
- `cc42d89`, PR #6 — harness batch 4:
  - UI reviewers must run the rendered output, with a guard against touching live sessions;
  - a file snapshot around each Implementer dispatch, which reports and restores any file the Implementer deleted;
  - a hook guarding against `claude.md` file names;
  - Codex retries once on capacity errors;
  - the harness prompts now state the GlenError/thiserror-only convention;
  - 5 gaps marked resolved.

  Its own review was blind and asked for changes (4 should-fix, 7 nits), all applied.
- Code index refreshed from `main` after each merge.

## Decisions and records (Mnemosyne, project glen)

- PR memories: `b6d77d60` (#5), `2a4c14af` (#6).
- Notes: W2 plan `633e767b`, checkpoint 2 `0ab6da6e`.
- Queue items W1 `c8e41f68` and W2 `6c17717c` updated with the path each wave took.
- Session controls (W3b, board `758ff5f9`) count as original scope, per the owner; no decision memory was written for them.

## Harness friction this hour

- **Review files named `claude.md` were loaded as instructions.** macOS's file system ignores case, so `reviews/w1/claude.md` loaded as a nested CLAUDE.md and put a review into the orchestrator's context as "project instructions". Renamed, and guarded in PR #6.
- **Codex review "model at capacity".** The first W1 Codex review failed and its retry succeeded. PR #6 now makes that retry standard.
- **A W1 Implementer deleted an untracked orchestrator file** (`.claude/launch.json`). PR #6 adds the file snapshot. The W6 fix pass is now deleting W6's own vendored morphin sources and `site/src/lib/utils.ts`; this gets checked before W6 merges.
- **The plan judge was not blind.** The owner flagged it. Fixed in PR #5; W2 was the first wave judged blind.
- **Both code reviewers missed the W6 demo defect;** only the visual check caught it. PR #5 added the visual gate; PR #6 makes reviewers run the output.

## Blockers

None.

## Next hour

- **W1:** merge fix cycle 1 and the conformance suite, run a scoped re-review, then PR, CI, memory, merge and close the board issue; re-index.
- **W6:** visual re-check (1280 light, 360 dark, Replay, reduced motion), then PR and merge.
- **W2:** rebase onto `main` after W1 merges, then the review gate (Claude and Codex, both running the CLI against the fake peer), conformance, PR.
- **W3:** plan the live route and extension; its protocol leaves room for W3b controls.
- **Harness batch 5:** whatever friction W1, W2 and W6 surface.
