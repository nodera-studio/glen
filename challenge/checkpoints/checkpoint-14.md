# Checkpoint 14 — hour 14 (14:01Z → 15:01Z, 2026-10-30)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.

## Wave ledger

| Wave | State |
| --- | --- |
| W0, W1, W2, W6, W7 | Merged (PRs #3, #9, #10, #7, #12). Site live at https://glen.nodera.workers.dev |
| W3 Live route + extension | **Merged: PR #16 → `381a031`** (memories `1ee405bf`, `eab7a8ae`). Board done + `shipped:main`. Worktrees are removed, their Codex brokers stopped, and the index re-run. |
| W4 Agent surface | Code is on main (`9f5a67e`). **Acceptance attempt 1 failed in harness, not in glen** (evidence `w4-acceptance/20261030T1438`, memory `b5a615aa`). Modes a and b3 died at setup: the omp TUI under `screen` never drew anything or accepted input. Mode b2 was a false negative from the checker regex. An investigator is root-causing it; then the harness gets fixed and all six modes are re-run. |
| W3b Session controls | **In progress** (worktree `w3b`). Codex completed steps 1–2. A resumed run is doing steps 3–11 and will be followed by host re-verification and a WIP commit. W3b owns the shared surface lists. |
| W5 `glen watch` | **In progress** (worktree `w5`). Deps `49893c6`, steps 1–2 `a690f21`; Codex is on step 3 (state, reducer, prompt, single keymap table). Its controls stay off until it merges W3b after W3b's review gate. |
| W9 | Pending, after W5. |
| **W10 Tech-debt paydown** | **New: owner directive.** All tech debt is fixed before the deployment waves (board `de08958c`, decision `fce2ca4d`). There are 9 items; item 6 was closed by W3's `7729bad`. |
| W8 Audits + v0.1.0 | Pending, after W10. |

## Landed on `main` this hour

- `1e76dd0`, PR #17: **harness batch 8** (memory `55bf247e`). The Implementer reports failed attempts and has a retry cap. There is a check before stopping an agent, stashes are restored by SHA, fix briefs use line ranges, and parallel waves follow a shared-seam procedure. A blind review found 12 issues and all are fixed.
- `381a031`, PR #16: **W3**. A Claude agent integrated W4's 16 conflicted files, which is a recorded routing deviation. A scoped Claude ‖ Codex merge review led to the `7729bad` sandbox-mapping fix. 908 tests pass.

## Decisions

- **Owner:** all tech debt is tackled before the deployment waves. W10 is added between W9 and W8 (`fce2ca4d`).
- W5 plan: seven reference-study amendments adopted (plan section and Note `4b92a767` comment).
- W3b and W5 run in parallel. W3b owns the shared surface lists, and W5 merges W3b early, never rebasing. This is recorded in both plans.
- W3 and W4 merged order: the sandbox refusal comes first, then the live route, then `refuse_rpc`. Connect and cleanup denials map to `sandboxed` (`eab7a8ae`).

## Harness friction this hour

- **impl-coder-codex wrote the W3 merge-review fix by hand** instead of dispatching Codex. That is a routing violation; a Codex review of the commit compensated (its one finding was refuted). Candidate for batch 9.
- A **stale codex-companion task from 06:42Z** (8.5 h, the W2 de-flake job) was still alive and has been stopped. Batch 7's broker-stop rule covers brokers, but not orphaned `task` processes.
- The W3b Implementer did not report attempt 1 on its own (it ended cleanly, so no failure report was triggered), and it left the work uncommitted between runs. Mitigation: a WIP commit between Codex runs.
- The acceptance harness had gaps the offline self-test could not catch: there is no b2 fixture, and setup failures leave `calls.ndjson` missing.

## Blockers

None. Owner action still pending: the `CLOUDFLARE_API_TOKEN` repo secret for CI deploys.

## Next hour

1. Root-cause the acceptance `screen` failure, then have the Codex Implementer fix the harness: the startup, the checker regex, a b2 fixture, and a guard for a missing `calls.ndjson`. Then re-run modes a, b, b2, b3, c1 and c2. c2 gets a config backup, temporary network access, and a byte-compare restore.
2. W3b: finish steps 3–11, then the review gate, conformance, live control smoke on scratch sessions, and the PR.
3. W5: continue the steps. Once W3b passes its review gate, merge W3b into W5 and turn the controls on.
4. Harness batch 9: Implementer must always dispatch Codex; clean up orphaned companion tasks; WIP commits between runs.
