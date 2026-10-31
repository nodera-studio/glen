# Checkpoint 23 — hour 23 (07:10Z → 08:01Z, 2026-10-31)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
Checkpoints 20–23 are staged in the main checkout and go into the next wave PR.

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W4, W3b, W6, W7 | Merged. |
| W5 `glen watch` | Fix cycle 2, groups a–c done:<br>• `a7ceb2b`: rapid double signal and the restore race.<br>• `ef60daa` + `48167cb`: full-prefix xxh3 rewrite hash, v1 occurrence rebase, incremental appends, shared with `tail -f`.<br>• `8bce4df`: header refresh, the separator fix, picker preselection, inline control warnings.<br>Group d is running: restoring the item-22 stand-ins and the ac516 "mTC" assertion, plus the low items. |
| W9 omp engine | Fix cycle 2 is running (`fixes2.md`). **Orchestrator decision `fca05728`:** read containment moves to the OS. On macOS, omp runs under `sandbox-exec` with a deny-by-default profile and an allowlist built from real runs. On Linux it needs bwrap. Snapshot cleanup becomes case-insensitive. The guard hook is being rewritten to handle heredocs, reserved words and wrappers, nested `-c`, fail closed when it can't resolve a command, and allowlist verbs by profile. An fd-3 leak is also being fixed. Group (a), the sandbox launcher, is being reworked after host verification.<br>The live smoke still needs the owner's Cursor login. |
| W10 Tech-debt paydown | **Plan final** (Note `b64412ef`). Blind judge: base B = Claude, 8 grafts from A = Codex, 9 judge fixes.<br>Four verified flake root causes:<br>• `fake_ext` accepted sockets inherit non-blocking mode on macOS;<br>• `c2_7` reads a control file half-written by a plain write;<br>• lock deadline is checked before the first attempt;<br>• macOS EINVAL from setting a socket timeout after the peer closed.<br>The proof is 20 consecutive full runs on each OS. Branching waits for W5 and W9 to merge. |
| W8 Audits + v0.1.0 | Pending, after W10. |

## Landed on `main` this hour

None.

## Decisions

- **`fca05728`:** for W9, omp's read containment is enforced in the OS (`sandbox-exec` or bwrap), not by listing the paths omp reads at startup. The general lesson: a CLI agent can't be confined by tool policy alone, because discovery and config loading happen before any tool call.
- **`0173e3b3`, refined by the judge's open items:**
  - A broken stdout pipe exits with the outcome's own code; success exits 0.
  - On Linux, a different network namespace while a Codex sandbox marker is set makes the lease unknown, and RPC resume is refused. In the same namespace (escalated commands), behaviour is unchanged.
  - `ls`, `show` and `watch` report `unknown` in that case.
- **W10:** keep added items 16–17 and the label-triggered flake-proof workflow.

## Harness friction

- **W9 safety took three review rounds of "one more omp discovery path".** The fix was to change the containment layer rather than keep listing paths. This is the same lesson as the batch-9 process scripts: when patching keeps leaking, step down a layer.
- **The blind judges keep catching real errors in both planners.** Examples: W10 candidate A got the `c2_7` root cause wrong and its `--unshare-pid` would break endpoint liveness; B's drain step fails on macOS with EINVAL. Blind judging plus source verification is paying for itself.

## Blockers

- **W9 live smoke:** the owner needs to run `omp --profile glen-harness login cursor`.
- **CI deploys:** the owner needs to add the `CLOUDFLARE_API_TOKEN` repo secret.

## Next hour

1. **W5:** group d, a quick Codex re-check, then the PR, CI and merge. Run `remove-worktree.sh` and re-index.
2. **W9:** finish fix cycle 2 (sandbox negative tests must deny, positive tests must start omp), then a scoped re-review. Then the live smoke, if the owner has logged in.
3. **W10:** branch once W5 is on main. W9's files don't overlap W10's except the orchestrator prompt for item 4. Start group 1 (contracts) and the flake root-cause fixes.
