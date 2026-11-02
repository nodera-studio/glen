# Checkpoint 58 — 12:26Z → 14:51Z, 2026-11-02 (v0.2.0 run, hours 14–15)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0, W1, W2 | Merged (PR #35 `35b7522`, PR #36 `715b1ff`, PR #37 `0157285`). |
| v0.2 W3 omp acp spike (`1918d79f`) | **Merged** (PR #38 `affb069`); board `done` + `shipped:main`, queue item done, branch deleted, index refresh started. |
| v0.2 W4 warm worker (`36b5d618`) | **Planning interrupted.** The Claude and Codex planners were dispatched at 13:17Z and both died on a session usage limit (reset 16:50 Bucharest); no plan file exists. Resume or re-dispatch next. |
| W5–W7 | Pending. |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

## Landed on `main` this window

- PR #38 `affb069` — W3: hidden `__acp`, ACP client, `fake_omp acp`, 20 integration tests, 46 blind conformance specs, verdict GO (scoped).

## Decisions

- **W3 closed** after review rounds 1 and 2 (Codex round 2: close keeps the job deadline as the permission cutoff; malformed frames beat cancels), live E1–E3 on 2 real turns, CI green. ADR `169e6250`, PR memory `6208472f`.
- Checkpoint files are committed on the active wave branch; with `main` active, this one goes on the W4 branch (no direct pushes to `main`).

## Harness friction

- A session usage limit killed both W4 planners and left the loop idle for about 85 minutes with no signal (already on the batch-13 list).
- `sd` is not on PATH; `sed -i` is blocked by a hook (use Edit); `code_reindex` needs `repo: "glen"` (a name, not a path).
- Carried for batch 13: omp deadline and heading tolerance; stale `glen` binary fallback; `cargo` not on PATH; Codex planner result extraction; `## Unreleased` and "run it as a workflow" wording in harness docs; conformance writer's model trailer; chained commands must not push after a failed test step; cargo from Python on macOS 27 needs `SDKROOT` unset; conformance module order is alphabetical (fmt), not as the plan says.

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys. Open for the owner: whether glen's routing moves from Codex to omp; whether each harness batch and W7 get an upstream-to-agent-os step; the agent-os port session's questions.

## Next hour

1. W4: resume the two planners (or re-dispatch), anonymize, blind judge, judge edits, then the Implementer.
2. Keep an eye on usage limits; log any further stall.
