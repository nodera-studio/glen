# Checkpoint 61 — 17:17Z → 18:17Z, 2026-11-02 (v0.2.0 run, hour 18)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0–W3 | Merged (PR #35 `35b7522`, #36 `715b1ff`, #37 `0157285`, #38 `affb069`). |
| v0.2 W4 warm worker (`36b5d618`) | **Review fixes and docs landed; blind conformance in progress** on `feat/v020-wave-4-warm-worker` (head `498eb14`, not pushed). Review round 1 (12 items) and round 2 (Codex: `--force` group proof, `models` re-routing) are fixed with failing-first tests; docs §A21 and CHANGELOG committed (`7d81991`); my own gate and full suite were green at `9914082` (1981 tests). Left: blind conformance (`v2_warm`, plus the two `release` contract runners), my final gate and suite, PR, CI, memory, merge, closure. |
| W5–W7 | Pending. |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

## Landed on `main` this window

None.

## Decisions

- **J7 replaced the live smoke with a source read** (omp 18.14.0): the session-owner lease is a per-process kernel lock held until the process exits, and RPC mode dispatches each `prompt` as it arrives, so an idle RPC omp keeps its lease and accepts another prompt. To be cited in the PR and the ADR memory.
- **W4 review round 2:** a dead omp leader's surviving process group blocks `forced` (`release_timeout`, registry kept); `route_models` re-runs owner routing after cooling.
- Open note from the fixes: after a `--force` timeout the worker is already dead, so the next `glen release` answers `cleaned` without re-checking the group; and the `release_timeout` hint still suggests `--force`. Both go in the notes for W6.

## Harness friction

- Docs updater had no shell, so I ran its test filters and committed for it; implementer commits again carried the Opus trailer despite the brief.
- Codex stalled three times on W4 planning (its two review passes returned fine).
- Carried for batch 13: omp deadline and heading tolerance; a stale omp result file reads as "failed" mid-run; stale `glen` binary fallback; `cargo` not on PATH; Codex planner extraction; harness-doc wording; chained commands must not push after a failed test step; cargo from Python on macOS 27 needs `SDKROOT` unset; `sd` not on PATH, `sed -i` and `cat` blocked by hooks.

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys. Open for the owner: whether glen's routing moves from Codex to omp; the upstream-to-agent-os step; the agent-os port session's questions.

## Next hour

1. W4: conformance lands; my gate and suite; PR, CI, memory, merge, closure.
2. W5 (glen mcp, generated from docs/json.md): dual-engine plan.
