# Checkpoint 37 — hour 37 (21:01Z → 22:01Z, 2026-10-31)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
Checkpoints 25–37 are staged in the main checkout and go into the next wave PR.

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W5, W3b, W6, W7, site fix | Merged. `main` is at `7f37ec1`. |
| W10 Tech-debt paydown (PR #23, draft) | **The Ubuntu flake proof passes 20/20** at `5ef7bec`, the F11 SIGWINCH fix.<br>The macOS proof is still running.<br>All regular CI is green: both `check` jobs, `linux-sandbox`, `site` and `site-deploy`. |
| W9 omp engine (PR #25) | Ready. Codex approved cycle 7 (`0faacf7`) and CI is green. Merges right after W10. |
| W8 Audit fixes | Ready on its branch: `40c8830`, then the credits change `0685a49`, then the flake fixes `a489d6b`. Waits for W10. |
| Harness batch 10 | PR #24 is held for W9 and phase B. |
| v0.1.0 release | Approved by the owner. Dry run 1 passed. |

## Landed on `main` this hour

None.

## Decisions

- **Scratch cleanup:** the owner allowed deleting the scratch folders, but the session's permission rules still deny `rm -rf`. The agent didn't work around the rule and gave the owner the exact command instead.

## Harness friction

- **An owner's chat approval can't override a permission deny rule.** That is correct, but it costs the owner a manual step for routine scratch cleanup. Batch-11 candidate: an allow rule scoped to `rm -rf "$TMPDIR/glen-*"`.
- **The macOS flake proof is the slow leg**, at about 20 minutes or more for 20 sequential full runs. That leg alone sets when W10 can merge.

## Blockers

None. The owner should rotate the temporary Cloudflare token and the R2 keys.

## Next hour

1. **W10:** the macOS proof passes 20/20, then the merge memory, the squash merge, the board update and the re-index.
2. **W9:** merge `main` in, run CI, merge. Then batch-10 phase B on PR #24 and merge.
3. **W8:** merge `main` in, open the PR, run CI, merge, and redeploy the site.
4. **Release PR:** README and CHANGELOG, then dry run 2, then the `v0.1.0` tag.
