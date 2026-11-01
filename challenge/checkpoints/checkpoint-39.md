# Checkpoint 39 — hour 39 (23:01Z, 2026-10-31 → 00:01Z, 2026-11-01)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
This checkpoint and checkpoint 38 go into the release PR (#27).

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W7, W3b, W9, W10, site fix | Merged. |
| W8 Audit fixes | **Merged** as PR #26 → `5e579a8`. Merging `main` after W9 in brought one docs conflict in `docs/json.md` and two compile breaks, all fixed: the `Warning` import, and `thinking_flag` in `tests/w8_jobs.rs`. `--thinking` is now in the `glen send` row of `design.md`. Every CI job, including `msrv`, passed on the first run. Memory `0c598d5a`. Board `054b1fbc` is tagged `shipped:main` and stays `in_progress` until the release. |
| Harness batch 10 | **Merged** as PR #24 → `29b215b`. Phase B covers B1 (orchestrator routing and the AC source check), B2 (index and reference docs) and B3 (Linux-CI review rule, verifiers, quick-classify). Claude ‖ Codex review: 1 HIGH, 6 MED and the LOWs, all fixed. `ac1027` is un-ignored; conformance passes 666/666. Memory `86014114`. |
| v0.1.0 release | **PR #27 opened** (`chore/release-v0.1.0`, `6a8456f`): `CHANGELOG.md` checked against `--help`, checkpoint 38 and the gap check. Dry run 2 and CI are in progress. |

## Landed on `main` this hour

- PR #26 (W8) → `5e579a8`.
- PR #24 (harness batch 10) → `29b215b`.

## Decisions

- **Merge order:** W8, then #24. `git merge-tree` showed the two branches merge cleanly, and `main` has no branch protection, so #24 merged without another CI round after W8 landed. `main`'s CI after the merge checks the combination.
- **The W8 board issue stays `in_progress`.** Its scope includes the release, so it closes when `v0.1.0` is published.
- **`coder-codex.md` wording kept:** "Codex writes code" is accurate only when `routing.sh` prints `codex`, which is the condition that prompt runs under.

## Harness friction

- **The workflow's integration output tripped the `settings-json` instruction-pattern filter**, because it quoted `settings.json` paths. The result was benign, but the filter neutralised its control tags. A filter that flags harness file paths will keep firing on harness batches.
- **`cargo +1.89` can't link on macOS 27:** the SDK's `.tbd` architecture `arm64e.x1` is rejected. The MSRV can only be checked in CI on this machine.
- **The plan's W9 `merge-tree` acceptance check went stale** once W9 landed as a squash. Plans that gate on an open branch need a "once merged, check against `main`" fallback.

## Blockers

None. The owner should:
- rotate the temporary Cloudflare token and the R2 keys;
- delete the scratch folders `$TMPDIR/glen-flake-*` and `$TMPDIR/glen-w9-accept7`.

## Next hour

1. **Release PR #27:** dry run 2 and CI pass, then the release memory and the squash merge.
2. **Tag:** the annotated `v0.1.0` on the squash commit (owner-approved), then watch `release.yml` and verify the 9 assets and the install script.
3. **After the release:** the `releaseAvailable` flip PR with the pre-release Playwright tests, the site redeploy, the `released:v0.1.0` board tags, and the final report with the harness evaluation.
