# Checkpoint 60 — 16:17Z → 17:17Z, 2026-11-02 (v0.2.0 run, hour 17)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0–W3 | Merged (PR #35 `35b7522`, #36 `715b1ff`, #37 `0157285`, #38 `affb069`). |
| v0.2 W4 warm worker (`36b5d618`) | **In the review-fix cycle** on `feat/v020-wave-4-warm-worker`. Implementer steps 1–11 done (`9320431`, gate and 1967 tests green on the Implementer's run). Review round 1 (Claude ‖ Codex ‖ omp) is in; fixes for items 1, 11 and 2, 3, 12 are committed (`a748904`, `847f3cc`); items 4–10 are in progress. Still mine: my own gate and suite re-run, the omp source read replacing the live smoke (J7), docs §A21 and CHANGELOG, the blind conformance module (with the two `release` contract runners), PR. |
| W5–W7 | Pending. |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

## Landed on `main` this window

None.

## Decisions

- **W4 review round 1.** Two-engine must-fix (Claude + Codex): the `send` status write erasing a warm claim; `release` cleanup and wait racing a successor worker. Taken single-engine: `--force` reporting success without proving termination (J11), ambiguous `env_digest` encoding, `cool_for_control` waiting out a whole job, a timing-flaky AC-403 test, the `conclude` retry, the refused-claim warm window, small doc/help nits. From omp (advisory, status `failed: malformed output`): `glen watch` marking a stolen job lost, and `release <@new job handle>` missing the worker after a follow-up. Both were forwarded for verification. Not changed: Codex's P3 on latency measurement (noted).
- The commit trailer on Implementer commits said Opus (the brief asked for Sonnet); commits are unpushed, so the squash message carries the right one.

## Harness friction

- Codex stalled three times on W4 planning; its review pass did return (456 s). Logged for batch 13 with the case for routing Codex to omp.
- The omp review's stale `.json` from the previous wave read as "failed" while the new run was still going: check `elapsed_ms` and file mtimes before trusting it.
- Carried for batch 13: omp deadline and heading tolerance; stale `glen` binary fallback; `cargo` not on PATH; Codex planner extraction; harness-doc wording; conformance writer's model trailer; chained commands must not push after a failed test step; cargo from Python on macOS 27 needs `SDKROOT` unset; `sd` not on PATH, `sed -i` and `cat` blocked by hooks.

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys. Open for the owner: whether glen's routing moves from Codex to omp; the upstream-to-agent-os step; the agent-os port session's questions.

## Next hour

1. W4: fixes finish; my gate and full-suite re-run; scoped re-review.
2. J7 source read, docs updater ‖ blind conformance writer, PR, CI, memory, merge, closure.
