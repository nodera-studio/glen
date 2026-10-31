# Checkpoint 35 — hour 35 (19:01Z → 20:01Z, 2026-10-31)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
Checkpoints 25–35 are staged in the main checkout and go into the next wave PR.

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W5, W3b, W6, W7, site fix | Merged. `main` is at `7f37ec1`. |
| W10 Tech-debt paydown (PR #23, draft) | **All regular CI is green** (both `check` jobs, `linux-sandbox`, `site`, `site-deploy`) after fix cycle 3:<br>• `5babb00`: Step 23 docs and lease detail;<br>• `e594136`: AC-410/412 migrated to AC-1028's Linux lease key under the cross-wave rule.<br>Flake proof, first attempt: macOS run 3 failed in `ac514`. The fix was cherry-picked from W8 (`1857069`). Flake proof rerun in progress. |
| W9 omp engine (**PR #25 opened**) | **Step 11 live acceptance: all 8 items pass:**<br>• the live review flagged the planted `24h` bug as HIGH;<br>• the debug run named the filter line;<br>• Mnemosyne `memory_search` returned a real memory;<br>• the real gate header showed `Claude ✓ · Codex ✓ · omp ✓ cursor/gemini-4-argon`;<br>• the skip path works;<br>• the default profile fingerprint is unchanged.<br>**Fix cycle 6** (`ca93d5e`, `225e5ce`): the sandboxed omp died on an EPERM spooling RPC output to `$TMPDIR`, fixed with a private TMPDIR. `~/.omp/run` exposure (HIGH) narrowed to `session-owners`.<br>Re-review: Claude PASS (9 LOW); Codex REQUEST CHANGES (P1 symlinked profile root; P2 commit identity in the fingerprint; P2 MCP ownership).<br>**Fix cycle 7** `0faacf7` pushed; CI green. The final report is pending. |
| W8 Audit fixes | `40c8830`, then the owner's credits change (`0685a49`), then the ac514/ac506 flake fixes (`a489d6b`). Waits for W10. |
| Harness batch 10 | PR #24 is held for W9 and phase B. |
| v0.1.0 release | Approved. Dry run 1 passed. |

## Landed on `main` this hour

None. PR #25 (W9) opened.

## Decisions

- **ac514 fix (TUI prompt layout cache):** a product change found by the flake investigation (`.claude/research/watch-live-flakes.md`). It lands on W8 and is cherry-picked into W10 so the flake proof can pass.
- **AC-410/412 migration:** the W4 blind tests gain only the Linux `details.lease:"unknown"` key, with a comment citing W10 fixes2 decision 1. Every other assertion is unchanged.
- **W9 cycle 7:** a symlinked profile path is refused, not followed. `mcp.json` is replaced only when it exactly matches a known glen template.
- **`live.rs` F10 (ac316/323/326):** left open with a diagnosis (fake extension descheduled past the 300 ms / 2 s probe deadlines). The only fixes on offer would widen deadlines.

## Harness friction

- **A stale `astro preview` from Oct 30 still holds port 4321.** Playwright's `reuseExistingServer` then silently tests the wrong build. Batch-11 candidate: the e2e config refuses to reuse a server from another checkout.
- **A hook blocked the investigator from deleting its own 3.9 GB of scratch build directories.** The owner was told the cleanup command. Candidate: give agents a `$TMPDIR`-scoped cleanup helper.
- **Concurrent `cargo build --example` briefly deletes example binaries,** so test suites sharing one target dir fail with "No such file". Rule: never run test suites at the same time in one target dir.

## Blockers

None. The owner should:
- rotate the temporary Cloudflare token and the R2 keys;
- optionally delete `$TMPDIR/glen-flake-{target,orig,dummy}`.

## Next hour

1. **W10:** the flake proof passes 20/20 on both OSes, then the merge memory, merge, board update and re-index.
2. **W9:** the cycle-7 report, then merge `main` (with W10) into it, CI, merge. Then batch-10 phase B on PR #24, and merge both.
3. **W8:** merge `main` in, open the PR, CI, merge. The site redeploys with the credits change.
4. **Release PR:** README and CHANGELOG, then dry run 2, then the `v0.1.0` tag.
