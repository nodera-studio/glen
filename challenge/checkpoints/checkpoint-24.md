# Checkpoint 24 — hour 24 (08:01Z → 09:01Z, 2026-10-31)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W4, W3b, W6, W7 | Merged. |
| W5 `glen watch` | **[PR #21](https://github.com/nodera-studio/glen/pull/21) is open** (memory `845ddaba`).<br>• Fix cycle 2 finished at `b8fdf1c`. Fix cycle 3 (`0d29efd`) covered three things: signal coalescing through a counter in the signal handler, the race between terminal entry and restore, and making an unchanged picker choice a no-op.<br>• Docs and checkpoints 20–23 are in `bc9fc7a`.<br>• **CI is red on macOS, twice in a row**, in `controls::ac356_ac364` (abort of a job-driven session: a count came out 0, expected 1).<br>• `watch::ac516` failed once and passed on the rerun, so it is flaky.<br>• Ubuntu is cancelled by fail-fast each time macOS fails.<br>• An investigator is root-causing both. |
| W9 omp engine | Fix cycle 2, groups a and b are done:<br>• `46d2379` macOS sandbox-exec read containment;<br>• `5ecc77c` Linux bwrap fallback;<br>• `294afec` guard rewrite: verb allowlist, heredocs, nested `-c`.<br>Groups c and d are running: fd 3, case-insensitive hygiene, ancestor checks, and the low items.<br>The live smoke still waits on the owner's Cursor login. |
| W10 Tech-debt paydown | Plan final (Note `b64412ef`). Branches after W5 merges. |
| W8 Audits + v0.1.0 | Pending, after W10. |

## Landed on `main` this hour

None.

## Decisions

- **W5:** dropped live result pairing, because AC-504 requires watch's streamed items to equal `tail -f`. An unchanged picker choice is now a no-op. Signals the kernel merges are documented: press Ctrl-C again.
- **W5 blind test `ac516`:** migrated to pick a model that differs from the current one, so the real setter-frame assertion still holds under the new no-op.

## Harness friction

- **W5's macOS CI fails in a conformance test that passes on the host every time.** CI's slower machines expose timing differences. The W10 flake fix, event-driven test waits, is overdue, and this PR is blocked on that same class of issue.
- **An approval I gave in chat never reached the agent.** I wrote the "docs line about kernel coalescing" in my reply but did not send it, and caught it at PR time. Every instruction to an agent must go through SendMessage.

## Blockers

- **PR #21 CI (macOS):** root cause in progress.
- **W9 live smoke:** waiting on the owner to run `omp --profile glen-harness login cursor`.
- **CI deploys:** waiting on the owner to add the `CLOUDFLARE_API_TOKEN` repo secret.

## Next hour

1. Fix the `ac356_ac364` macOS failure (and `ac516` if it turns out to be real), then push, get CI green, merge #21, remove the worktree and re-index.
2. Branch W10. Its first group is the flake root-cause fixes, which unblock CI stability for every later PR.
3. W9: finish fix cycle 2, then a scoped re-review.
