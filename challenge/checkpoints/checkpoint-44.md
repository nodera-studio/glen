# Checkpoint 44 — 17:59Z → 19:00Z, 2026-11-01 (v0.2.0 run, hour 1)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`. Goal: glen v0.2.0, ending with tag v0.2.0. The headline is that omp replies reach the calling agent without polling `glen inbox`. Four additions ship with it:
- an omp version range and nightly CI against the latest omp;
- an `omp acp` spike;
- an opt-in warm worker;
- an MCP layer generated from `docs/json.md`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | **Done.** Explorer and deep researcher ran first. Claude and Codex architects each proposed an approach, and a blind judge chose. The spine is the Claude approach "wake-on-exit on a synchronous core" with no new crates, plus 3 Codex grafts. Plan: `.claude/plans/v020.architecture.md`. ADR memory `0773c35d`. |
| v0.2 W0 foundation (`3c9ac8f1`) | **Implementing** on `feat/v020-wave-0-foundation` (`c6adaa6`: steps 1–4). Docs and tests only. Plan: `.claude/plans/v020-wave-0-foundation.md`. ADR memory `3d8d582b`. |
| W1 omp compat + nightly (`c1ea2657`) | Pending. |
| W2 replies without polling (`c5190ea3`) | Pending (headline). |
| W3 ACP spike (`1918d79f`) | Pending. |
| W4 warm worker (`36b5d618`) | Pending. |
| W5 `glen mcp` (`37706789`) | Pending. |
| W6 audits and debt (`0ae5e983`) | Pending. |
| W7 release v0.2.0 (`6ed2f0fe`) | Pending. |
| Harness batch 12 | **PR #33** (head `883a4f8`): the omp engine becomes a third `/code-review` pass. Codex review found 2 MED and 1 LOW, all fixed. CI is running. |

## Landed on `main` this hour

None.

## Decisions

- **What "push" means per caller.** No first-party mechanism wakes an already-running, idle Claude Code or Codex session. So v0.2 delivers a reply as a one-shot receiver the caller starts itself.
  - Claude Code: a background task wakes the session.
  - Subagents: a foreground wait.
  - Codex: a blocking wait. This is not push, and the release copy will say so.
- **Autonomous defaults, recorded in the state decision log (18:36Z):**
  - Channels stays a hidden flag.
  - The warm worker holds glen's lock, with a 10-minute maximum.
  - The range is 18.14.0–18.14.0 until the nightly passes on a newer omp.
  - The receiver uses the existing `wait` and `inbox` commands; no new verb.
- **Per-wave step plans** are written at each wave's bootstrap against current main. Seam rule: a wave may amend an earlier §A section only where its own behaviour changes it.
- **CHANGELOG:** each wave adds a bullet under `## Unreleased` this run.
- **The owner said "no more workflows"** for the v0.2 run: plain agent-os Agent dispatch only (memory `no-workflow-tool`).

## Harness friction

- `/code-review` was Claude ‖ Codex only, so omp never reviewed wave code. Fixed in PR #33.
- The owner noticed that batches 1–12 changed only glen's installed `.claude/`, never the agent-os repo. A separate session is now porting the generic changes to agent-os.
- `docs-updater.md:32` allows CHANGELOG edits only in release PRs, which conflicts with per-wave `## Unreleased` bullets. Open, for batch 13.
- The harness docs still say to run harness batches and fix groups "as a workflow". Open, for batch 13.
- **Session restart, about 18:57Z.** It stopped every session-local process: the W0 Implementer (mid-wave, after steps 1–4), the PR #33 CI watcher, and the hourly cron. The project agent types did not reload either. Recovery: the Implementer was resumed from its transcript at step 5, and the cron was re-created as `54412a65`. Session-only crons and background agents can't outlive a restart, so after any restart the run must recheck them.

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys.

## Next hour

1. Finish W0: Implementer, deliberate re-run, Role 2 review (three engines, omp included), AC source check, conformance and docs tail, then the PR and merge.
2. Land PR #33 when CI is green. Remove its worktree.
3. Bootstrap W1 with a dual-engine step plan.
