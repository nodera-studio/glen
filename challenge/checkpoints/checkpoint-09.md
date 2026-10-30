# Checkpoint 9 — hour 9 (09:01Z → 10:38Z, 2026-10-30)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.

## Wave ledger

| Wave | State |
| --- | --- |
| W0, W1, W2, W6, W7 | Merged (PRs #3, #9, #10, #7, #12). Site live at https://glen.nodera.workers.dev |
| W3 Live route + extension | Fix cycle 1 is merged into the wave branch (`1974a58`). It adds per-request settlement, fixes session-switch handling, hardens installer symlink/TOCTOU checks, and stops replies being lost. TS tests are 31/31. Conformance was re-derived (`dcc8db3`): 377/377, including 107 W3 rows. Smoke step 5 was **re-run on real omp**: the follow-up and the steer now get their own answers. The re-run also showed that an answer arriving after omp's async-result wake-up was dropped. The span rule is now refined: spans close at user messages, and a notice closes a span only after it has an answer. Checked against both live traces. **Fix cycle 2 is running.** |
| W4 Agent surface | Fix cycle 1 and fix cycle 2 have landed. Sandbox detection: only `CODEX_SANDBOX` counts as an early refusal, and Linux detection is now failure-driven. `install-skill` gets TOCTOU safety. The drift test is stronger. Conformance was re-derived to 351/351, and `main` is merged in (`5c7c1c3`, 729 tests green). The Codex confirm-review found 2 narrow sandbox-mapping P2s: omp's `FailedToOpenSocket` is unmatched, and provider 403s are mislabelled. **Fix cycle 3 is running**, then the PR. |
| W5 `glen watch` | **The plan is final, judged blind.** Base B (Claude) plus 8 grafts from A (Codex), plus judge fixes. Decisions 1–6 are recorded (note `4b92a767`). Starts after W3 merges. |
| W3b Session controls | The plan is final. Starts after W3. The W5 merge-order note is on the board issue. |
| W9 / W8 | Pending |

## Landed

Nothing merged to `main` this hour; all three active waves are in fix and verify cycles. The site preview was started for the owner at localhost:4321.

## Decisions and records (Mnemosyne, project glen)

- `c395a6b8`: confirmed omp facts.
  - `sendUserMessage` returns no message id.
  - A bash tool started while the model is busy gets backgrounded, and an async-result wake-up follows.
  - Records the span rule both live traces confirm.
- W3:
  - O6 refinement (span rule).
  - 9 conformance clarifications.
  - `not_delivered` job error code.
- W4: 6 cycle-2 clarifications.
  - `OMP_` and `PI_` agent-dir aliases.
  - Sandbox-marker write errors map to `sandboxed`.
  - Newer edits win.
- W5: decisions 1–6. A second signal exits at once, drafts are kept per session, and the lockfile growth is accepted.
- **Owner approvals:**
  - **W4 c2:** temporary Codex network access, with backup and byte-compare restore.
  - **CI deploy secret:** yes. Creating the token is an owner-only credential step, and the instructions have been given.

## Harness friction this hour

- **codex-rescue can only write inside its cwd**, and that cwd follows the orchestrator shell's working directory. So the W5 planner couldn't save into the main checkout. I worked around it by having it return the plan text. Logged `[open]`.
- **Implementer sandbox false negatives.** Codex's sandbox reported socket-bind EPERM and git `index.lock` failures in every Implementer run. The Claude host re-ran everything on the host and committed. This works, but every run repeats it, so it needs a batch-7 note.

## Blockers

None. The CI deploy secret waits on the owner.

## Next hour

- **W3:** fix cycle 2, then re-run smoke step 5, scoped re-review, PR and merge.
- **W4:** fix cycle 3, then PR and merge. Acceptance runs after W3 is on `main`: modes a, b, b2, b3, c1 and c2.
- **W3b and W5:** implement once W3 is on `main`.
- **Harness batch 7:** codex-rescue writes outside its cwd; documented host verification for Implementer sandbox artifacts.
