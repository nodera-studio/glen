# Checkpoint 69 — 02:23Z → 03:27Z, 2026-11-03 (v0.2.0 run, hour 27)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0–W4 | Merged (PR #35 `35b7522`, #36 `715b1ff`, #37 `0157285`, #38 `affb069`, #39 `d1708f0`). |
| v0.2 W5 glen mcp (`37706789`) | **Merged** (PR #40 `582f77b`); board `done` + `shipped:main`, queue item done, branch deleted, index refresh started, memory `96dcfd76`. Live legs: Claude Code passed 7/7 (5 turns, $0.41; the client probed `server/discover`, got -32601, then `initialize` 2025-11-25); Codex owner-pending; the interactive Channels check (AC-505d) is an owner step. |
| v0.2 W6 audits (`0ae5e983`) | **In progress** on `feat/v020-wave-6-audits`. Dispatched on `main`: `omp-latest` (run `41571361463`, completed success) and `flake-proof` (run `41571364482`, in progress). Docs audit done (0 critical, 0 high, 4 medium, about 20 low; report `.claude/reviews/v020-docs-audit.md`). Tech-debt audit running. Security audit held until those report. Left: the fix pass, AC-601 decisions, flake-proof result, AC-604 acceptance re-run, PR. |
| W7 | Pending (release v0.2.0). |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

Progress against the goal: plan plus six of eight waves merged or nearly so; W6 audits then the release remain (about 75%).

## Landed on `main` this window

- PR #40 `582f77b` — W5: `glen mcp`, 14 tools generated from `docs/json.md`, hand-rolled synchronous server, hidden `--channel`; 2202 tests pass, CI green on macOS and Linux.

## Decisions

- Docs-audit findings to fix before the release: M1 `docs/design.md` §A19 and two comments in `src/omp/compat.rs` claim the site reads `compat.json` (it hardcodes `testedOmp`); M2 site copy has no v0.2 content (`wait --for reply`, `inbox --wait`, `glen mcp`, `--warm`, the supported range, the exit-10 `woke` case, version string); M3 `.claude/CLAUDE.md` dependency list drift; M4 `.claude/agents/README.md` slash-command table. Site code goes to an Opus agent.
- `omp-latest` on `main` passed: whether a newer omp than 18.14.0 should raise `max_tested` is read from its summary (AC-603) before the fix pass.
- `ci` on `main` at the W4 merge (`d1708f0`) ended with `check (macos-latest)` **cancelled**, not failed (Linux, sandbox, msrv, site all passed; the PR run on the same head was green); the push run at `582f77b` is in progress and decides whether it repeats.

## Harness friction

- A push-triggered `ci` run on `main` can be cancelled without a code failure; verify per job, do not read the run-level `failure` as a regression.
- Codex planner stalls (four across W4 and W5); its review passes return.
- Carried for batch 13: omp deadline and heading tolerance; stale `glen` binary fallback; `cargo` not on PATH; Implementer commit trailers sometimes say Opus; usage limits idle the run silently; checkpoint commits need a fresh branch after a merge; chained commands must not push after a failed test step; cargo from Python on macOS 27 needs `SDKROOT` unset; `sd` not on PATH, `sed -i` and `cat` blocked by hooks.

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys; the Codex MCP leg and the interactive Channels check (AC-505c, d) are owner steps. Open for the owner: whether glen's routing moves from Codex to omp; the upstream-to-agent-os step; the agent-os port session's questions.

## Next hour

1. W6: tech-debt audit report, omp-latest summary, flake-proof result; start the security audit; one fix pass (docs/site/code) for the findings taken.
2. AC-604 acceptance re-run on the final main, then W7 (release v0.2.0).
