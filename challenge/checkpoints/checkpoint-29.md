# Checkpoint 29 — hour 29 (13:01Z → 14:01Z, 2026-10-31)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
Checkpoints 25–29 are staged in the main checkout and go into the next wave PR.

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W5, W3b, W6, W7 | Merged. |
| Site fix | **Merged: PR #22 → `7f37ec1`.** The redundant header divider is gone. The owner asked for no decision record, so only a minimal merge memory (`f79e7016`) exists, to satisfy the hook. |
| W9 omp engine | **Model ladder** `6b73c58`: argon (medium) → 3.8-flash (medium) → grok-4.7 (high), set per dispatch with a new `glen send --thinking` flag.<br>**Sandbox DNS** `e9b8373`: found through the Sandbox deny log. A real sandboxed `glen send @new "reply ok"` returns `ok`.<br>Cycle-3 re-reviews:<br>• Codex: 2 P1 (launcher inside a child-writable dir; Linux abstract sockets).<br>• Claude: 1 HIGH (profile write protection bypassable by renaming the dir; loadable files can be newly created), 2 MED.<br>**Fix cycle 4 is running** (`fixes4.md`). |
| W10 Tech-debt paydown | Steps 1–20 done, written by Claude after F1–F5:<br>• `6f88264` atomic swap for install-extension and install-skill;<br>• `cc59a90` Linux lease-hidden refusal, narrowed to a different network namespace;<br>• `2f8dec7` light `accent_2` → `#0E7C70` / xterm 23.<br>Remaining: the bwrap job, README items, and item 4, which waits for W9. |
| W8 Audits + v0.1.0 | Pending. |

## Landed on `main` this hour

- `7f37ec1`, PR #22: the site header divider fix.

## Decisions

- **W9 cycle 4, orchestrator decisions**, recorded in `fixes4.md`:
  - The omp engine is **unavailable on Linux** in v0.1.0. bwrap can't isolate abstract sockets while keeping network access.
  - Profile writes switch to an allowlist, and renaming the agent dir or the profile root is denied.
  - Outbound network is limited to TCP 443 plus the mDNSResponder socket.
  - Residual exfiltration risk is documented: any HTTPS host is reachable, and `agent.db` is readable.

## Harness friction

- **A sandbox deny-list protects only paths that exist at launch.** Renaming or creating paths gets around it. An allowlist plus a deny on the directory itself is the sound shape. Same lesson again: when in doubt, allowlist.
- **Silent sandbox denials cost about 2 hours of DNS debugging.** The Sandbox deny log (`log stream --predicate 'sender == "Sandbox"'`) found the cause in minutes. It should be the first step for any sandbox issue.

## Blockers

None. The owner should rotate the temporary Cloudflare token and the R2 keys.

## Next hour

1. **W9:** fix cycle 4, then a scoped re-review, then the step-11 live smoke (a real omp review and debug pass through glen), then the PR.
2. **W10:** the bwrap job and README, a local stress run, and the review gate (Claude ‖ Codex). Item 4 comes after W9.
3. **W8:** plan the audits.
