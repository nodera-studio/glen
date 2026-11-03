# Checkpoint 66 — 23:20Z → 00:21Z, 2026-11-03 (v0.2.0 run, hour 24)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0–W4 | Merged (PR #35 `35b7522`, #36 `715b1ff`, #37 `0157285`, #38 `affb069`, #39 `d1708f0`). |
| v0.2 W5 glen mcp (`37706789`) | **Review-fix cycle** on `feat/v020-wave-5-mcp`. Implementer steps 1–8 done (`91e595b`); my gate and `--no-fail-fast` suite on it: 2120 passed, 0 failed. Review round 1 (Claude ‖ Codex ‖ omp, omp status `ok` this time) is merged: must-fix (two engines) = `jsonrpc` envelope never validated, blocking stdout write stalls lifecycle handling, channel seen-set eviction replays history; plus single-engine items taken (stdout purity under `--json`, orphaned children on EPIPE, overflow reader, dead cancelled-result arm, throttle arithmetic, baseline signal, tool descriptions, test hardening). The fix Implementer is running. Docs (§A22, README, CHANGELOG) committed. Left: fixes land, my gate and suite, blind conformance (`v2_mcp`), my live legs, PR, CI (Linux-only code needs a green `check (ubuntu-latest)`), memory, merge. |
| W6–W7 | Pending. |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

## Landed on `main` this window

None.

## Decisions

- **Rejected review findings (with reasons):** answering `initialize` with the newest version for an unsupported request is spec-correct (a server answers a version it supports and the client decides); the `--channel` baseline stays inside `initialize` (binding J6: otherwise a scripted client races the first reply).
- **Linux-only code** (`/proc/self/exe` fallback, dash/procps-dependent tests) is settled by the PR's Linux CI, not a fix cycle.
- Live legs (`claude -p` against `fake_omp`, capped at $1.00 and three tools; Codex owner-pending unless a no-config route exists) wait until after the fixes and conformance.

## Harness friction

- Codex planner stalled on both W5 and W4 (four in a row); its review passes return.
- The omp review returned `ok` for the first time since W2 (4.3 minutes); the earlier "failed" reads were a stale result file, so the result file is now cleared before each run.
- The docs updater has no shell, so I run its test filters and commit for it.
- Carried for batch 13: omp deadline and heading tolerance; stale `glen` binary fallback; `cargo` not on PATH; Implementer commit trailers sometimes say Opus; usage limits idle the run silently; checkpoint commits need a fresh branch after a merge; chained commands must not push after a failed test step; cargo from Python on macOS 27 needs `SDKROOT` unset; `sd` not on PATH, `sed -i` and `cat` blocked by hooks.

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys; the interactive Channels check (AC-505d) is an owner step. Open for the owner: whether glen's routing moves from Codex to omp; the upstream-to-agent-os step; the agent-os port session's questions.

## Next hour

1. W5: fixes land; my gate and suite on a quiet machine; blind conformance writer; my live legs.
2. PR, CI, memory, merge, closure; then W6 (tech-debt and pre-release audits).
