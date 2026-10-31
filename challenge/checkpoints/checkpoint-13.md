# Checkpoint 13 — hour 13 (13:20Z → 14:01Z, 2026-10-30)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
Checkpoints 8–12 are on the W3 branch (PR #16) and reach `main` with it. This file is staged with the next wave PR, because the W3 worktree is mid-merge.

## Wave ledger

| Wave | State |
| --- | --- |
| W0, W1, W2, W6, W7 | Merged (PRs #3, #9, #10, #7, #12). Site live at https://glen.nodera.workers.dev |
| W4 Agent surface | **Code merged, PR #15 → `9f5a67e`** (memory `ad88a5b9`). Four fix cycles, the last two being narrow sandbox-matcher edges (`FailedToOpenSocket`, `ETIMEDOUT`, glen-framed os errors only). Conformance 351/351, CI green. Board item is `in_review` + `shipped:main` while the **acceptance runs** wait for W3 on `main` (modes a, b, b2, b3, c1, and c2 with temporary Codex network, which the owner approved). |
| W3 Live route + extension | PR #16 is open. Fix cycle 3 is `95c8e5b`: it keeps the last non-empty assistant text and makes install-extension create files exclusively (Codex confirm review). After W4 merged, `main` conflicts with this branch in **16 files** where both waves touched the same seams (`send.rs`, `doctor.rs`, `cli.rs`, conformance surface rows). **An Opus integration agent is resolving it**, keeping both sides' intent: W4's sandbox refusal runs before W3's endpoint connect, the visible surface is the union of both, and the skill renders the W3 commands. Live smoke passed 11/11 before the merge. |
| W5 `glen watch` | Plan is final. **A reference study is running** at the owner's suggestion, covering codex-rs/tui (same ratatui stack), omp's own `packages/tui`, and opencode's TUI. It will produce amendments to the W5 plan before implementation. |
| W3b, W9, W8 | Pending. W3b and W5 start after W3 merges. |

## Landed on `main` this hour

- `e1bb3ee`, PR #14, **harness batch 7**: codex-rescue returns its artifacts as text; the host re-verifies Implementer sandbox artifacts and is the only one that commits; removing a worktree stops its Codex broker. Memory `05fc5d5c`.
- `9f5a67e`, PR #15: **W4** agent surface (memory `ad88a5b9`).

## Decisions

- W4: provider authorization errors stay `omp_error`. Only glen-framed permission errors and omp transport errors map to `sandboxed` when a Codex marker is set.
- W3: `assistant_text` is the last non-empty assistant text in the span. Install-extension creates new files with an exclusive rename. The replace window is documented as a residual risk.
- **Routing deviation (recorded):** integrating two already-reviewed waves went to a Claude Opus agent, not the Codex Implementer.

## Harness friction this hour

- **Parallel waves that share seams make one large merge at PR time.** Logged: the parallel-wave procedure should name shared seams up front, and either run those waves one after the other or rebase early.
- Two Codex confirm reviews each found further narrow edges (W4 matcher, W3 span text). That is diminishing returns, but each edge was real and cheap to fix.

## Blockers

None. Owner action pending: CI deploy secret.

## Next hour

1. Finish the W3 integration, then CI, merge, close the board item and re-index.
2. W4 acceptance runs: all six modes, including c2 with the backup, temporary network, and byte-compare restore.
3. Amend the W5 plan from the reference study, then implement W5 and W3b in parallel. Their seams are disjoint: `src/watch` versus `src/ctl`, with a W5 adapter.
4. Harness batch 8: silent-retry reporting, checking a worktree's diff and stash before stopping an agent, line ranges for small fixes, and the shared-seam procedure.
