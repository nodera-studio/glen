# Checkpoint 63 — 20:17Z → 21:18Z, 2026-11-02 (v0.2.0 run, hour 21)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0–W3 | Merged (PR #35 `35b7522`, #36 `715b1ff`, #37 `0157285`, #38 `affb069`). |
| v0.2 W4 warm worker (`36b5d618`) | **PR #39 open, CI running** (head `c201c2c`). All review rounds are fixed: round 1, then four Codex scoped re-reviews (the tombstone protocol was replaced by a per-session registry metadata lock; release and teardown now report lock failures honestly). My own gate and `cargo test --locked --no-fail-fast` on the head: 2054 passed, 0 failed. CI: Linux, linux-sandbox, msrv and site green; macOS pending. Left: CI green, memory (separate call), merge, board/queue closure, index refresh, sweep. |
| W5–W7 | Pending. |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

## Landed on `main` this window

None.

## Decisions

- **Registry cleanup:** a per-session metadata flock (serializes registry and `.release` publication, conditional removal, teardown removal; lock file unlinked when it guards nothing, with an inode re-check) replaced the rename-to-tombstone protocol after Codex found three races in it.
- **Honest failure reporting:** `glen release` reports `cooling` only once the request is written; a lock timeout on cleanup is exit 7 `release_timeout` with the files kept; teardown has one 3.6 s deadline and a single lock acquisition for both removals.
- Accepted residuals, documented in §A21: a release sent before a worker's first registry publish finds nothing; a stopped metadata-lock holder can leave files behind until its release.

## Harness friction

- A session usage limit stopped the Implementer and idled the run for an hour (second occurrence).
- Codex review passes took four rounds on W4's concurrency code; each found real defects, which is the cost of the metadata-lock design.
- One full-suite run with a Codex review in parallel timed out a control test (`ac352`); it passed alone and in later quiet-machine runs. Rule kept: run my gate and suite with no other agents active.
- Implementer commits sometimes carry the Opus trailer; the squash message corrects it.
- Carried for batch 13: Codex planner stalls; omp deadline and heading tolerance; stale omp result file reads "failed" mid-run; stale `glen` binary fallback; `cargo` not on PATH; chained commands must not push after a failed test step; cargo from Python on macOS 27 needs `SDKROOT` unset; `sd` not on PATH, `sed -i` and `cat` blocked by hooks.

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys. Open for the owner: whether glen's routing moves from Codex to omp; the upstream-to-agent-os step; the agent-os port session's questions.

## Next hour

1. W4: macOS CI, memory, merge, closure, index refresh.
2. W5 (glen mcp, generated from docs/json.md): dual-engine plan (retry Codex; single-candidate judge if it stalls), then the Implementer.
