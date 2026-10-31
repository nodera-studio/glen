# Checkpoint 25 — hour 25 (09:01Z → 10:01Z, 2026-10-31)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
This file is staged in the main checkout, because the W5 worktree has an agent's uncommitted edits in progress. It goes into the next PR.

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W4, W3b, W6, W7 | Merged. |
| W5 `glen watch` | PR #21 is open. **The macOS CI failures have root causes:**<br>• `ac356_ac364` is a **product race in `glen abort`**, present since W3b. The queue report is taken after the abort answer, and by then the next queued job may already hold the lock. The fix is to snapshot the queue before dispatch, verified with a forced 200 ms pause.<br>• `ac516` is a **test-peer bug**. fake_ext writes each log line in several syscalls, so lines tear when two writers overlap, and `records()` drops the broken lines without saying so. The fix is a single `write_all` per line, plus `records()` failing loudly on a malformed line.<br>The fix is in progress. It will be pushed to PR #21. |
| W9 omp engine | **The owner logged in to Cursor** in `glen-harness`.<br>Live probes pass:<br>• 80 Cursor models are listed, including inside the sandbox.<br>• Mnemosyne read tools work.<br>• Writes, shell and URL reads are refused.<br>• `glen_reply` works.<br>• Setup runs for real and is idempotent.<br>• The default-profile fingerprint is unchanged.<br>**Owner model choice:** `cursor/gemini-4-argon`, falling back to `cursor/gemini-3.8-flash` (`fda1152`).<br>Cycle 2 re-reviews: Codex REQUEST CHANGES (7 × P1: an unsandboxed preflight, profile extensions writable from inside the sandbox, no fail-closed without sandbox-exec, Unix-socket egress, and guard bypasses); Claude 5 × MED (a guard performance cliff that fails open, plus false positives).<br>**Fix cycle 3 is running** (`fixes3.md`). |
| W10 Tech-debt paydown | Plan final. Addendum: flake causes F6 and F7 (fixed in W5), and a rule that stress runs never build examples in parallel. |
| W8 Audits + v0.1.0 | Pending. |

## Landed on `main` this hour

None.

## Decisions

- **Owner:** the W9 model is `cursor/gemini-4-argon`, then `gemini-3.8-flash`. Memory `4796b17a` is updated.
- **`8297b5a5`:** retire the shell-parsing guard hook and use best-effort `permissions.deny` rules instead. The real guarantees are profile-only invocations, a default-profile fingerprint that also covers the WAL/SHM files, and the OS sandbox.
- **The CLOUDFLARE_API_TOKEN secret stays an owner action.** Creating a token and entering it into a secret is credential handling, which the agent never does. The owner was given the dashboard steps and the `gh secret set` command.

## Harness friction

- **Third time the same lesson.** The batch-9 process scripts, the W9 discovery-path enumeration and now the W9 shell guard all followed one pattern: patching leaks, and each review finds a new bypass. Recorded in `8297b5a5`: narrow the component to what can be enforced, or remove it.
- **A product race hid as a "flake" for two waves.** The abort race existed since W3b. Local runs always passed and only the slow macOS CI runner exposed it. Do not label CI-only failures as flakes before forcing the timing.

## Blockers

- **PR #21:** pending the CI fix push.
- **CI deploys:** the owner still needs to add the `CLOUDFLARE_API_TOKEN` repo secret. The steps were given in chat.

## Next hour

1. **W5:** push the CI fix, get CI green, merge #21, run `remove-worktree.sh`, re-index.
2. **W10:** branch from main once W5 is in. Group 1 is the flake root causes (F1–F7) and the contracts.
3. **W9:** finish fix cycle 3, then a scoped re-review, then the Step 11 live smoke (one real omp review and one debug pass through glen), then the PR.
