# Checkpoint 30 — hour 30 (14:01Z → 15:01Z, 2026-10-31)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
Checkpoints 25–30 are staged in the main checkout and go into the next wave PR.

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W5, W3b, W6, W7, site fix | Merged. |
| W9 omp engine | Fix cycle 4 is in progress, uncommitted in the W9 worktree: the launcher, profile setup, engine script, permission rules, skill, and `send --thinking` validation. After it: scoped re-review, the step-11 live smoke, then the PR. |
| W10 Tech-debt paydown | **Steps 1–20 passed 10 of 10 local full test runs.**<br>**Steps 18–19** `457205d`: bwrap scenario suite and a `linux-sandbox` CI job (AC-1020–1024, AC-1028).<br>**Review gate on steps 1–20** (Claude ‖ Codex): BLOCK.<br>• Claude: 2 HIGH, 6 MED, 10 LOW.<br>• Codex: 2 P1, 5 P2.<br>• Both found the same two serious bugs: the `/proc` namespace check used `lstat`, and install-skill's swap cleanup could unlink a competitor's file.<br>**Fix cycle 1** `f3d1eb7`, `fa64a7f`:<br>• namespace comparison by target, with a loopback-only fallback;<br>• swap ownership tracking;<br>• hidden-lease display in `ls`, `show` and `watch`;<br>• drain deadline;<br>• legacy `no_route` reason;<br>• inbox rescan cap.<br>Next: the docs pass, then the push and a draft PR so Linux CI verifies the bwrap suite. |
| W8 Audits + v0.1.0 | Pending. |

## Landed on `main` this hour

None.

## Decisions

All W10 fix-cycle-1 decisions are recorded in `.claude/plans/v010-wave-10-tech-debt.fixes.md`. Mnemosyne memories follow at merge.
- **Hidden-lease rule:** a Codex marker is set AND either the namespace targets differ, or `/proc/1/ns/net` is unreadable and the namespace is loopback-only. If `/proc/self/ns/net` itself can't be read, fail closed.
- **Swap temp ownership:** never unlink a displaced file unless its dev/ino proves it is ours.
- **Publish vs. fsync:** publication and fsync durability are reported separately.
- **Legacy jobs:** a `no_route` job without a reason is normalised to a documented fallback reason.
- **Linux CI before review:** W10 opens a **draft PR before the review gate closes**, because the bwrap suite can only be compiled and run on Linux CI. Clippy for the Linux target type-checks it locally. The PR is not marked ready until the gate passes.

## Harness friction

- **The implementer misread "Claude writes the code" as a deviation from its own prompt.** `impl-coder-codex.md` still says Codex writes everything. It flagged its work correctly, but the agent prompt contradicts the current routing memory `148655d9`. Batch-10 candidate: make the routing a single setting that both the agent prompt and the orchestrator read.
- **Code that only runs on Linux can't be verified on the macOS host.** Clippy for the Linux target catches type errors, but not the runtime behaviour of bwrap or namespaces. Only CI verifies it, so the review gate for such code needs a CI run first.
- **The "local stress run is green" claim covered code that had two real bugs, including one certain to fail on Linux.** Running a test 10 times on one OS proves it isn't flaky; it doesn't prove the code is correct. The dual review caught both bugs.

## Blockers

None. The owner should rotate the temporary Cloudflare token and the R2 keys.

## Next hour

1. **W10:** the docs pass, then push and open a draft PR, iterate on Linux CI, then a scoped re-review of `457205d..` (Claude ‖ Codex). After that, the flake-proof label run (N=20 per OS) and the conformance tail.
2. **W9:** finish fix cycle 4, then a scoped re-review, then the live smoke, then the PR.
3. **W8:** plan the audits once W9 and W10 are merged.
