# Checkpoint 8 — hour 8 (08:01Z → 09:01Z, 2026-10-30)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`. State file: `.claude/auto-mode/state.md`.

## Wave ledger

| Wave | State |
| --- | --- |
| W0, W1, W2, W6 | Merged (PRs #3, #9, #10, #7) |
| W7 Deploy and release | **Merged, PR #12 → `69630d9`.** The site is live at https://glen.nodera.workers.dev and was redeployed from `main` after the merge (version `297f8704`). The release dry run is green on all 4 targets, twice. The board item is done and tagged `shipped:main`. |
| W3 Live route and extension | Implementer `0df763c`, 587 Rust tests and 19 TS tests. **Live smoke against real omp 18.14.0 TUIs under `screen`: 10 of 11 steps pass.** The idle-TUI gap is closed. The live prompt and reply work with no fork. Talk-back typed in the TUI reaches the inbox. `GLEN_MANAGED` works both ways. Quit and crash cleanup work, and so does the exit-6 refusal when there is no extension. **Step 5 fails:** when deliveries interleave (prompt, follow-up, prompt, steer, async wake-up), answers are attributed to the wrong jobs. This is the plan's O6 gate. The settlement rule was amended to per-request message-boundary attribution. Review gate: Claude approved with notes (3 medium, 11 low); Codex requested changes (4 P2). Conformance is `00b6c69` (370/370, 100 new W3 tests, including real-extension-under-Node tests); 9 ambiguities were settled. **Fix cycle 1 is running.** |
| W4 Agent surface | Implementer `1a60027`. Review gate: Claude returned REVIEW_REQUIRED (1 high, 3 medium, 12 low); Codex requested changes (4 P2, 1 P3). The high finding: the Codex sandbox flag also appears on escalated commands, which would refuse the fix the skill itself recommends. Conformance is `2b7d4e5` (349/349). **Fix cycle 1 is running**; it writes 10 spec clarifications first. Acceptance runs come after W3 merges. |
| W3b Session controls | Planned and judged blind; starts after W3 merges. |
| W5 / W9 / W8 | Pending |

## Landed on `main` this hour

- `69630d9`, PR #12: **W7** release machinery and workers.dev deploy (memories `4beeb0ec`, `821a462e`).
- `981b660`, PR #13: **harness batch 6**. `steer-bash` now allows glen's own site, and every URL must be on the allow-list. Only a numeric port may follow the host, and the whole command is scanned (memory `33eba5c2`). A blind review asked for 3 should-fix changes; all are applied.

## Decisions and records (Mnemosyne, project glen)

- **W3 O6 outcome:** the settlement and attribution amendment. A request's answer is the span from its own user message to the next boundary, so a submission that never reached omp is never matched to an unrelated run. Batched requests share a span and carry a `batched` warning. The 9 conformance clarifications are recorded in the W3 plan.
- **W4:**
  - The sandbox counts as present only when `CODEX_SANDBOX` is non-empty.
  - `install-skill` re-validates its target right before writing.
  - `--dry-run` never writes, including spill files.
  - 10 spec clarifications.
- **W4 O1:** the owner's omp credentials are never copied. Acceptance uses scratch-cwd buckets and a before/after snapshot of the sessions folder.

## Harness friction this hour

- **Fixed in #13:** the project's own site was blocked for live checks.
- No new agent misbehaviour. A Codex implementer's self-report wrongly blamed the sandbox for unix-socket failures; the Claude host re-ran the checks and found the real cause, O(n²) buffer handling. The cross-check worked as intended.

## Pending owner decisions (non-blocking)

- **W4 acceptance mode c2:** may I temporarily set `network_access = true` in `~/.codex/config.toml`, with a backup and a byte-for-byte restore?
- **CI deploys:** they need the `CLOUDFLARE_API_TOKEN` repo secret.

## Blockers

None.

## Next hour

- **W3:** merge fix cycle 1. Re-derive the conformance rows. **Re-run live smoke step 5** on the host. Scoped re-review, then PR and merge.
- **W4:** merge fix cycle 1. Re-derive conformance. Re-review, then PR and merge. Run acceptance once W3 is on `main`: modes a, b, b2, b3 and c1, plus c2 if you say yes.
- **W3b:** implement once W3 is on `main`.
