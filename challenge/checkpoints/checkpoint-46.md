# Checkpoint 46 — 20:17Z → 21:18Z, 2026-11-01 (v0.2.0 run, hour 3)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done (`.claude/plans/v020.architecture.md`). |
| v0.2 W0 foundation (`3c9ac8f1`) | **Merged** as PR #35 → `35b7522`; board done + `shipped:main`; branch deleted. |
| v0.2 W1 omp compat (`c1ea2657`) | **Planning.** Branch `feat/v020-wave-1-omp-compat`. Claude and Codex step plans are written; a blind judge is selecting between them. |
| W2–W7 | Pending. |
| agent-os port (nodera-studio/agent-os PR #2) | Open, not merged. Its session was told about the grok-only ladder and the owner's direction to replace the Codex leg with omp. |

## Landed on `main` this hour

- PR #35 → `35b7522`: W0 foundation. Review gate: Claude ‖ Codex ‖ omp (grok-4.7 high). Fixes in `2b55417` and `79054fa`. 1595 tests pass.

## Decisions

- **W0 review:** the omp pass ended `failed: malformed output` (a text preamble before its heading), so it carried no vote. Its three MEDIUM findings were each verified against the code and applied as documentation clarifications (the `aborted` causes in §A7, the two exit-10 documents in §A20, the queued-for-warm-worker liveness grace in §A21).
- **W1 planning finding:** a bare real omp 18.14.0 in a scratch HOME exits "No default model selected" before it sends `ready`, so AC-v2-105 and AC-v2-106 cannot be provider-free as written. The Claude plan proposes a `models.yml` that declares a loopback stub provider (no credentials, no network). The judge is checking this.
- **Merge mechanics:** a merge attempted in the same tool batch as the memory store was refused by the capture-before-merge hook. Store the memory in an earlier call, then merge.

## Harness friction

- The omp engine discards a good review when anything precedes its heading. Open for batch 13.
- The omp engine fell back to a stale `target/release/glen` when `glen` was not on PATH. The profile setup re-run rebuilt it; preferring the newer build, or failing with a stale-binary reason, is open for batch 13.
- After an account switch, `cargo` was missing from PATH. Open: the harness scripts could add `~/.cargo/bin` themselves.
- After a restart, a resumed Implementer's id does not resolve; a fresh one with the same brief is the workaround.
- Still open for batch 13: the docs-updater and release skill don't know about `## Unreleased`; the harness docs still say "run it as a workflow".

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys; decide whether glen's own routing should also move from Codex to omp, and whether to add the upstream-to-agent-os step to each harness batch and to W7.

## Next hour

1. W1: take the judged plan to `/implement`: implementer, review gate (Claude ‖ Codex ‖ omp), AC check, conformance tail, PR, CI, merge.
2. Check the first `omp-latest` run on the W1 PR through its own-path trigger.
3. Start W2 (the headline: `wait --for reply`, `inbox --wait`, `task_state`, skill).
