# Checkpoint 17 — hour 17 (17:01Z → 18:01Z, 2026-10-30)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.

## Wave ledger

| Wave | State |
| --- | --- |
| W0, W1, W2, W3, W6, W7 | Merged: PRs #3, #9, #10, #16, #7, #12. The site is live at https://glen.nodera.workers.dev. |
| **W4 Agent surface** | **Done.** Code: PR #15. Acceptance: **PR #18 → `66c4325`** (memory `aa3d022a`). All six modes pass; board done, queue done. |
| W3b Session controls | Implementation-blind conformance suite: `5376277`, 69 cases. Every acceptance criterion that can be tested offline passes, except the real defect **D1**: under load the extension sometimes holds the socket open after answering. Fix cycle 1 is running in groups. Group (a), `906ff57`, closes the duplicate control at job end, the inbox deadline, the job_ended mapping and the "job finishing" message. Groups (b) through (e) are still to do: the ephemeral cancel leak, stderr, the TUI compact false success, D1, contract runners, and the low-severity items. |
| W5 `glen watch` | `2c228da` covers steps 6–7: the controls stub, the runtime loop and the CLI wiring. `e82f2f0` covers step 8: `watch` becomes visible, with its own `json.md` section and one guarded skill sentence. Step 9 (snapshot suite and purity guard) is running. |
| W9 | Pending, after W5. |
| W10 Tech-debt paydown | 14 items. Item 10 (flakes under load) now includes `ac326` (macOS CI, PR #18) and `node_ac307`. It may share a cause with D1. |
| W8 Audits + v0.1.0 | Pending, after W10. |

## Landed on `main` this hour

- `66c4325`, PR #18: **W4 acceptance**. The harness is fixed across six live runs, a dual review and a re-review. Every relaxed check has a fixture for both its passing and its failing side.

## Decisions

- W3b conformance readings R1–R6 are accepted. They are recorded in the fix brief; R5 means an endpoint that appears while a control waits for the lock is used live.
- b3 uses the scoped grant `--allowedTools "Skill(glen)"`. The host probe confirmed that Claude Code 2.1.309 accepts it.
- The checker reads the unfiltered transcript and fails closed on unknown line types that carry a tool use. Only the evidence copy is filtered.
- `lenient_errors` applies only in `evidence_shapes`. The `ac412` fixture contract stays strict.

## Harness friction this hour

- **I removed a worktree before stopping its Codex broker**, against the batch-7 rule; I stopped the broker right afterwards. Batch 9 will add one helper script so the order can't be skipped.
- CI fail-fast cancelled the Ubuntu job when the macOS `live.rs` flake hit. A single flake costs a full CI rerun until W10 de-flakes the tests.
- The W5 Implementer took five plain-reading questions from Codex. Each was correct, but each one cost a round trip. The plan's "If W4 is merged" conditionals should be resolved when the plan is written, not at implementation time.

## Blockers

None. Owner action still pending: the `CLOUDFLARE_API_TOKEN` repo secret for CI deploys.

## Next hour

1. W3b: finish fix groups (b)–(e). Then a scoped Claude ‖ Codex re-review, 10 consecutive green runs of the conformance `node_` cases, and a live smoke re-run of the compact case. Then open the PR.
2. W5: steps 9–12. After the W3b review gate, merge W3b into W5 and turn the controls on.
3. Harness batch 9:
   - the Implementer never hand-edits code and always dispatches Codex;
   - orphan cleanup of companion tasks;
   - WIP commits between long Codex runs;
   - a live smoke for any harness that drives a TUI;
   - one helper that stops the broker and removes the worktree;
   - resolve plan conditionals when the plan is written.
