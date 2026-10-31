# Checkpoint 26 — hour 26 (10:01Z → 11:01Z, 2026-10-31)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
Checkpoints 25 and 26 are staged in the main checkout and go into the next wave PR.

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W4, W3b, **W5**, W6, W7 | Merged. **W5: PR #21 → `e7d1966`** (memory `845ddaba`), board done, worktree removed through `remove-worktree.sh`. |
| W9 omp engine | Fix cycle 3: groups (a) `cfc4085` and (b) `f7f14a6` are done; group (c) is running.<br>• (a) Retired the shell-parsing guard and replaced it with permission deny rules.<br>• (b) Every omp call goes through the sandbox launcher; fail closed when no sandbox is available; the launcher and profile path have a single source.<br>• (c) Read-only executable config and `natives` inside the sandbox; IP-only egress plus the Unix sockets omp needs; narrower mach-lookup and process-exec; WAL/SHM fingerprint.<br>Next: a scoped re-review, then the Step 11 live smoke. |
| W10 Tech-debt paydown | In progress on `e7d1966`:<br>• `1931eb7`: flake-check script, label-triggered flake-proof workflow.<br>• `53fed35`: flake root causes F1–F5 — fake_ext blocking accept; live client drain/timeout order (macOS EINVAL); atomic control writes; lock acquire-first; margin policy.<br>Item 4 (the harness check of ACs against source) waits for W9 because of the shared `orchestrator.md`. |
| W8 Audits + v0.1.0 | Pending, after W10. |

## Landed on `main` this hour

- `e7d1966`, PR #21: **W5 `glen watch`**. The macOS CI fix (`d82df87`) closed a `glen abort` queue race that had existed since W3b, plus fake_ext log tearing.
- **First CI site deploy** with the owner's new `CLOUDFLARE_API_TOKEN`: the `site-deploy` job on the merge to main uploaded and deployed `glen`, and https://glen.nodera.workers.dev returns 200. The owner can now rotate the temporary token.

## Decisions

- **Credential handling stays with the owner.** The owner pasted a temporary token in chat. The agent refused to enter it, and the owner ran `gh secret set` themselves. The agent advised rotating it, since it is now in the chat log. CI then verified that the token works without the agent ever touching its value.
- **W10 seam:** item 4 (`orchestrator.md`) runs after W9 merges. Every other W10 group proceeds now.

## Harness friction

- **The owner pasted live credentials into chat**, including R2 S3 keys from the same screen. Those values are in the local session transcript. Mitigation: the agent never repeats or uses them, and the owner rotates. Batch 10 candidate: a pre-prompt hook that warns when a pasted message looks like a secret.

## Blockers

- None that block the run. The owner should rotate the temporary Cloudflare token and the R2 keys that were exposed in chat.

## Next hour

1. **W9:** group (c), then a scoped re-review, then the live smoke (one real omp review and one debug pass through glen), then the PR.
2. **W10:** remaining groups (contracts: EPIPE and `no_route` reason; `--cwd`; palette; atomic swap; bwrap job; README; install-skill), then the local stress run, then the PR. Item 4 follows after W9.
3. **Then W8:** audits and v0.1.0.
