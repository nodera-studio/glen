# Checkpoint 28 — hour 28 (12:01Z → 13:01Z, 2026-10-31)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
Checkpoints 25–28 are staged in the main checkout and go into the next wave PR.

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W5, W3b, W6, W7 | Merged. |
| W9 omp engine | Fix cycle 3 is complete (`cfc4085`, `f7f14a6`, `d99fa2a`):<br>• the shell guard is retired in favour of permission deny rules;<br>• every omp call goes through the launcher, and it fails closed without a sandbox;<br>• the profile's executable config and `natives` are read-only inside the sandbox, with SQLite directories still writable;<br>• outbound traffic is IP-only plus the required Unix sockets;<br>• `mach-lookup` and process exec are narrowed.<br>In progress: the owner's model ladder (argon @ medium → 3.8-flash @ medium → grok-4.7 @ high). Next: scoped re-review, then live smoke, then PR. |
| W10 Tech-debt paydown | Steps 1–14 done. The latest commit, `28ddaac`, makes `--cwd` scope handle resolution on show, tail, send and inbox (AC-1007–1012). Next: palette, atomic swap (install-extension and install-skill), namespace lease rule, the bwrap job and README. Item 4 waits on W9. |
| Site fix | **PR #22**, drop the redundant header divider. Done by Opus `impl-coder-ui`; verify passes and Playwright 49/49. CI is green and the PR is waiting for the owner to merge, since the owner asked for no memory, so the capture-before-merge hook blocks an agent merge. |
| W8 Audits + v0.1.0 | Pending. |

## Landed on `main` this hour

None.

## Decisions

- **Routing, three owner messages this hour.** Final: **Claude writes all code; Codex keeps review, planning, debug and audit roles** to stretch about 10% of weekly quota (`148655d9`, superseding `e5464a64`).
- **W9 model ladder, owner** (`4796b17a` updated): `cursor/gemini-4-argon` @ medium → `cursor/gemini-3.8-flash` @ medium → `cursor/grok-4.7` @ high.
- **Site divider fix, owner:** no memory or decision record, at the owner's request.

## Harness friction

- **Routing changed three times in 15 minutes.** Instructions were already queued to the running agents, so each change needed explicit "ignore my last message" follow-ups. Lesson: confirm a routing change before broadcasting it to agents that are mid-turn.
- **The capture-before-merge hook conflicts with the owner's "no memory" request** for trivial PRs. Candidate for batch 10: an explicit owner-waiver path, such as a merge-commit trailer or a waiver memory naming the PR.

## Blockers

- PR #22 needs the owner to merge it, or to allow a one-line memory so the agent can.
- The owner should rotate the temporary Cloudflare token and the exposed R2 keys.

## Next hour

1. **W9:** finish the model ladder, then scoped re-review (Claude ‖ Codex), the live smoke and the PR.
2. **W10:** remaining groups, written by Claude, then the local stress run and the review gate.
3. **W8:** plan the audits.
