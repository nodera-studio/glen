# Checkpoint 57 — 11:22Z → 12:26Z, 2026-11-02 (v0.2.0 run, hour 13)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0, W1, W2 | Merged (PR #35 `35b7522`, PR #36 `715b1ff`, PR #37 `0157285`). |
| v0.2 W3 omp acp spike (`1918d79f`) | **Built, reviewed twice, live-tested, conformance written; PR not yet open.** Branch `feat/v020-wave-3-acp-spike` at `641975f`, pushed through `f9bfd63`. Verdict GO, scoped. Left: AC source check, PR, CI, merge, board/queue closure. |
| W4–W7 | Pending. |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

## Landed on `main` this window

None.

## Decisions

- **W3 verdict: GO, scoped** (2026-11-02, 2 real model turns). `omp acp` is a viable optional transport for sends that need no `glen_reply`; `__acp` stays hidden. `session/load` neither holds nor checks the owner lease, so glen's pre-spawn guard is the only protection (it held, exit 6 `live_owner`). E2 never exercised the real-omp permission path (the model attempted no tool); the allow path is proven on `fake_omp` only. ADR memory `169e6250`.
- **Review round 2 (Codex):** two P2s, both real, fixed in `f9bfd63`: `session/close` keeps the job deadline as the permission cutoff; a malformed trailing frame is exit 7 `acp_malformed` even after a cancel (an existing test was changed on purpose).
- **Conformance (blind):** 46 specs in `tests/conformance/v2_acp.rs`, all pass; module registered before `v2_compat` (alphabetical, fmt requires it). Gaps recorded by the writer: no fake scenario for a permission request after cancel, an id-reusing agent request, the size caps, or a second signal.
- **Two `v2_compat` timeouts** (`ac_v2_105`, `ac_v2_106`) in my full-suite run at load average 25; both pass alone. Treated as load flakes; CI decides.

## Harness friction

- Carried for batch 13: omp's deadline and heading tolerance; a usage limit stops the loop silently; stale `glen` binary fallback; `cargo` not on PATH; Codex planner result extraction; `## Unreleased` and "run it as a workflow" wording in harness docs; the conformance writer's model trailer; chained commands must not push after a failed test step; cargo from Python on macOS 27 needs `SDKROOT` unset.
- New: `sd` is not on PATH (used Edit instead); the full suite on a loaded machine times out two timing-sensitive conformance tests; the plan said to register the conformance module between `v2_compat` and `v2_w0`, but fmt wants it before `v2_compat`.

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys. Open for the owner: whether glen's routing moves from Codex to omp; whether each harness batch and W7 get an upstream-to-agent-os step; the agent-os port session's questions.

## Next hour

1. W3: AC source check, push, PR, CI, separate memory call, merge, close the board issue and queue item, index refresh, sweep.
2. Start W4 (the opt-in warm worker) with a dual-engine step plan.
