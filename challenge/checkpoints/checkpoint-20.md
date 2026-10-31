# Checkpoint 20 — hour 20 (20:01Z → 21:01Z, 2026-10-30)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
This file is staged in the main checkout and goes into the next wave PR, because all three wave worktrees have agents writing in them.

## Wave ledger

| Wave | State |
| --- | --- |
| W0, W1, W2, W3, W4, W6, W7 | Merged. Site live at https://glen.nodera.workers.dev |
| W3b Session controls | **PR #20 open** (memory `4e50511b`).<br>• Fix cycle 2: `3a03418` and `3534b04`. A mutation that was already submitted is never retried (`outcome_unknown`). Ctrl-C during preflight never submits. Startup stderr is now surfaced. Teardown stops omp first. The final Codex re-check was clean.<br>• Main merged in as `9a9725a`. One trivial `json_contract.rs` conflict was resolved by keeping both sides.<br>• CI: macOS green. **Ubuntu red** on `ac362_r4`: Linux caps a single argv at 128 KiB (`MAX_ARG_STRLEN`), so a 524 KB `--instructions` argument can't be spawned. A platform-aware fix that keeps the same assertion is in progress. |
| W5 `glen watch` | Fix cycle 1, groups a–d done:<br>• `e9c5503` signals and dead terminal<br>• `18c9b55` ASCII hints, AC-520 states, help overlay<br>• `220d09c` follow and rewrite detection, shared with `tail -f`, which closes W10 item 1<br>• `ec00867` colour, path, timezone, graphemes, Ctrl-M<br>Group (e), the remaining low-severity items, is running. |
| W9 omp engine | **Owner decisions** (`4796b17a`): Cursor, a `glen-harness` profile, a read-only third reviewer and debugger.<br>• Research: `f592475d`.<br>• Blind-judged plan: base A (Claude), 6 grafts, 10 judge fixes; Note `83876461`.<br>• The Codex Implementer has started: `2b8b542` (steps 1–2).<br>• **Live smoke waits on the owner:** `omp --profile glen-harness login cursor`. |
| W10 Tech-debt paydown | 14 items; item 1 will close with W5. |
| W8 Audits + v0.1.0 | Pending, after W10. |

## Landed on `main` this hour

None. Three waves are in progress in parallel.

## Decisions

- **Owner (W9, `4796b17a`):**
  - Cursor subscription.
  - Dedicated `glen-harness` profile, so the default omp config is never touched.
  - Third reviewer and debugger only; no supervisor role.
- **W9 judge open items, decided by the orchestrator** and recorded in the plan and Note `83876461`:
  - The live smoke runs in the real profile with scratch dirs only.
  - omp runs in every review gate, with a `GLEN_OMP_ENGINE=off` kill switch.
  - `restrictedFolders: ["~"]`.
  - `~/.omp/run/` holds runtime state, not config, and is disclosed.
- **W5 placeholder layout.** The empty-prompt placeholder sits on its own row. The inline form broke a blind conformance assertion (`watch.rs:4689-4715` via `input_text`), so the extra row was chosen and that test left unchanged.
- **W5 grapheme widths.** Graphemes are measured the way ratatui measures whole graphemes (`UnicodeWidthStr`).

## Harness friction this hour

- **A blind conformance test hit a Linux-only argv limit.** Tests developed and run on macOS can still fail on Linux CI. Plans and conformance writers should check OS limits whenever a test passes large argv values.
- **The W9 planner output tripped a harness "instruction-shaped" filter** (settings-json), because it quotes `.claude/settings.json` paths. It was handled as data and needed no action.
- **Every wave worktree is busy at the same time,** so checkpoints have to be staged in the main checkout.

## Blockers

- **W9 live smoke:** the owner must run `omp --profile glen-harness login cursor` once.
- **CI deploys:** the `CLOUDFLARE_API_TOKEN` repo secret is still pending from the owner.

## Next hour

1. **W3b:** land the Linux fix, get CI green, merge #20, close the board item, clean up through `remove-worktree.sh`.
2. **W5:** finish group (e). Merge main (with W3b) into W5, turn the controls on, restore the strict completions assertions and add the control snapshots. Run Step 12 by hand. Then a scoped re-review, then the PR.
3. **W9:** continue the step groups offline; run the live smoke once the owner has logged in.
4. **Then W10.**
