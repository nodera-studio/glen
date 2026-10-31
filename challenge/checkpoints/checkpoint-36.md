# Checkpoint 36 — hour 36 (20:01Z → 21:01Z, 2026-10-31)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
Checkpoints 25–36 are staged in the main checkout and go into the next wave PR.

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W5, W3b, W6, W7, site fix | Merged. `main` is at `7f37ec1`. |
| W10 Tech-debt paydown (PR #23, draft) | All regular CI is green.<br>**Flake-proof round 1** at `1857069`: Ubuntu run 5 hit a new flake, **F11** `resizing_switches_narrow_wide_and_too_small_layouts`.<br>**Root-cause fix** `5ef7bec`: product code. `watch` now registers SIGWINCH before the first frame; before, a resize that arrived early was lost.<br>**Flake-proof round 2** is in progress on both OSes. |
| W9 omp engine (PR #25) | **Ready.**<br>• Fix cycle 7 (`0faacf7`): refuses a symlinked profile path, puts the commit id in the snapshot fingerprint, and replaces `mcp.json` only when it is a known template (by sha256).<br>• **Codex closure check: APPROVE.** P1 and both P2s are closed; no new defects.<br>• CI green.<br>• The live review was rerun on the committed HEAD: it caught the planted bug, with the script hashes recorded.<br>• Memory `6fd91ceb`.<br>Merges right after W10, once `main` is merged into it. |
| W8 Audit fixes | `40c8830`, then credits `0685a49`, then the flake fixes `a489d6b`. Waits for W10. |
| Harness batch 10 | PR #24 is held for W9 and phase B. |
| v0.1.0 release | Approved by the owner. Dry run 1 passed. |

## Landed on `main` this hour

None.

## Decisions

- **F11 is a product fix, not a test fix.** A SIGWINCH that arrived before the handler was registered was dropped. The handler is now registered before the first frame.
- **W9 cycle-7 decisions** are stored in memory `6fd91ceb` (project glen), together with the omp 18.14.0 fact that the native loader always reads `~/.omp/natives`.

## Harness friction

- **The flake proof keeps finding one new real race per round:** F8, F9, then ac514, then F11. Each round costs about 20 minutes of CI. The label-triggered N=20 proof is doing its job, but it is the critical path to merge. Batch-11 candidate: a nightly N=20 on `main`, so these surface before a wave's merge gate.
- **Cleanup commands (`rm -rf` on `$TMPDIR` scratch) are blocked for agents.** Scratch clones and build dirs pile up for the owner to delete.

## Blockers

None. The owner should:
- rotate the temporary Cloudflare token and the R2 keys;
- optionally delete `$TMPDIR/glen-flake-*` and `$TMPDIR/glen-w9-accept7`.

## Next hour

1. **W10:** flake proof round 2 passes 20/20 on both OSes. Then the merge memory, the merge, the board update and the re-index.
2. **W9:** merge `main` in, CI, merge. Then batch-10 phase B on PR #24 (the orchestrator.md routing and the AC-1027 source check, which un-ignores `ac1027`), and merge it.
3. **W8:** merge `main` in, open the PR, CI, merge. The site redeploys with the credits change.
4. **Release PR:** README and CHANGELOG, then dry run 2, then the `v0.1.0` tag.
