# Checkpoint 34 — hour 34 (18:01Z → 19:01Z, 2026-10-31)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
Checkpoints 25–34 are staged in the main checkout and go into the next wave PR.

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W5, W3b, W6, W7, site fix | Merged. `main` is at `7f37ec1`. |
| W10 Tech-debt paydown (PR #23, draft) | **Fix cycle 2 merged and pushed**, via 5 parallel groups (`dec1da0`, `4341b04`, `5cf5823`, `fcddede`, `44433fd`) plus the integrator's revert of the revoked orphan decision (`26afcdc`):<br>• blanket Linux hidden-lease rule;<br>• refusal while the glen lock is held;<br>• inbox cursor probe before output (the same approach as W8 G1);<br>• endpoint priority in `show`;<br>• absolute drain deadline;<br>• swap recovery paths;<br>• F9 fix (`LOCK_UN`);<br>• TUI test upper bounds restored.<br>Re-reviews: Claude PASS with notes; Codex REQUEST CHANGES (1 P2: the hidden-lease envelope lacks `lease:"unknown"` when `CODEX_SANDBOX` is set).<br>**Fix cycle 3** `5babb00`: Step 23 docs, the lease detail on every refusal, control refusal order, the drain edge case, CI runner pin, `ac1027` ignored until phase B. CI and the flake-proof rerun are in progress. |
| W9 omp engine | Acceptance workflow: re-reviews and Step 11 live acceptance done. **Fix cycle 6** `ca93d5e`: private omp TMPDIR, daemon-state containment, launcher containment, persisted thinking suffix. The PR step is next. |
| W8 Audit fixes | Done at `40c8830`. The owner's **credits change** (drop "(MIT)" after Morphin; link Nodera Studio to https://nodera.studio) is in progress on the W8 branch, by the Opus UI agent. It waits for W10 to merge. |
| Harness batch 10 | PR #24 is held for W9 and phase B. |
| v0.1.0 release | Approved by the owner. Dry run 1 passed. |

## Landed on `main` this hour

None.

## Decisions

- **The owner's site credits change goes on the W8 branch**, which already edits `Footer.astro`. A separate PR would conflict with it.
- **W10 LOW-007 stands as designed.** A failed stderr write after output exits 1, as `docs/design.md:364` specifies.
- **AC-1027 is `#[ignore]` in W10**, with the reason recorded. Phase B removes the ignore.

## Harness friction

- **The revoked decision cost one integrator commit (`26afcdc`), but no review round.** Writing the revocation into the brief worked, because the integrator re-read the brief before merging. Batch-11 candidate: every agent that commits re-reads the brief first.
- **Four W10 conformance failures turned out to be plan steps nobody owned:** the Step 23 docs and Step 21. The implementer reported them as "not done" but no fix cycle picked them up until the blind tests failed. The orchestrator should turn every "not done" item into a tracked step.

## Blockers

None. The owner should rotate the temporary Cloudflare token and the R2 keys.

## Next hour

1. **W10:** CI goes green at `5babb00` or later, the flake proof passes 20/20 per OS, then the merge memory, the merge, the board update and the re-index.
2. **W9:** PR opened and CI run. Then merge `main` (with W10) into it and merge. Then batch-10 phase B on #24, and merge both.
3. **W8:** merge `main` in, open the PR, CI, merge, redeploy the site with the credits change.
4. **Release PR:** README and CHANGELOG, then dry run 2, then the `v0.1.0` tag.
