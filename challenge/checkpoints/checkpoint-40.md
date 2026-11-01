# Checkpoint 40 — 00:01Z → 07:38Z, 2026-11-01

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
This window ran past an hour because the release and the site flip waited on agents. No wave branch
is open now, so this checkpoint goes into the closing checkpoints PR.

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W10, W3b, site fix | Merged. Every board issue is tagged `released:v0.1.0`. |
| v0.1.0 release | **Published.** Release PR #27 → `5279828` (memory `853909b3`). Dry run 2 was green (run 41254280899). The annotated tag `v0.1.0` → `5279828` was pushed. Release run 41255836934 was green and the release was published at 00:20:03Z. |
| Site flip | **Merged** as PR #28 → `97a4a9c` (memory `88e8d11e`). `releaseAvailable: true`; the install script is the default; the G7 e2e tests became post-release assertions; e2e 51/51; LHCI 1.00. The CI deploy on `main` was still running when this checkpoint was written. |
| W8 board issue | **Done.** Its scope included the release. |

## Landed on `main` this window

- PR #27, `chore: release v0.1.0` → `5279828`: `CHANGELOG.md`, checkpoints 38–39.
- Tag `v0.1.0`, release https://github.com/nodera-studio/glen/releases/tag/v0.1.0 with 9 assets.
- PR #28, `chore(site): flip to released v0.1.0` → `97a4a9c`.

## Decisions

- **Release notes:** replaced the generated PR list, which included the harness PRs, with the CHANGELOG v0.1.0 section plus install steps. The owner chose this in chat.
- **Verifying the install without `curl`:** the agent hook blocks `curl`. I used `gh release download` plus a checksum check and ran `glen --version`. CI's `verify-install` covered `install.sh` on both OSes, and the site agent's online link check covered the install URL.
- **Recorded in Mnemosyne:** release `853909b3`, publish and flip `88e8d11e`.

## Harness friction

- **The `steer-bash` hook blocks `curl`,** so the runbook's install smoke (`curl … install.sh | sh`) can't run as written. Batch-11 candidate: allow `raw.githubusercontent.com/nodera-studio/glen` in `ALLOW_HOSTS`, or rewrite the runbook step around `gh`.
- **Astro 7 sends `preview` to the background when it detects an agent,** which made Playwright report "exited early". The site agent worked around it with `--ignore-lock` in a temporary config. Candidate: put that in the site's Playwright config.
- **Flake on a macOS runner:** `tests/live.rs:925`, `show_and_tail_use_extension_state_for_persisted_sessions`. Tech-debt memory `28071a84`.
- **The checkpoint window stretched to 7.5 hours:** the run had no heartbeat while it waited on long agents. Batch 10 added ScheduleWakeup heartbeats, but this session did not use them.

## Blockers

None. The owner should:
- rotate the temporary Cloudflare token and the R2 keys;
- delete the scratch folders `$TMPDIR/glen-flake-*` and `$TMPDIR/glen-w9-accept7`.

## Next hour

1. Confirm the `main` CI deploy for `97a4a9c` and that the live site shows v0.1.0.
2. Finish the final report (`.claude/handoffs/v010-final-report.md`) and store the harness evaluation as an agent-os memory.
3. Open the closing PR with checkpoint 40 and any later checkpoints, then set the auto-mode state to complete.
