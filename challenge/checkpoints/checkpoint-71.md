# Checkpoint 71 — 04:28Z → 05:37Z, 2026-11-03 (v0.2.0 run, hour 29)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0–W5 | Merged (PR #35 `35b7522`, #36 `715b1ff`, #37 `0157285`, #38 `affb069`, #39 `d1708f0`, #40 `582f77b`). |
| v0.2 W6 audits (`0ae5e983`) | **Code and CI fixes committed; docs in flight** on `feat/v020-wave-6-audits` (head `5f0fb3a`, not pushed). Commits: `b063994` watch-frame teardown race; `443a765` SHA-pinned actions, read-only token, `--ignore-scripts`, https-or-file installer; `c5ca0e7` invisible Unicode and hostile session-id handles; `91e0505` private-cwd no-extension listing children, `O_NOFOLLOW`, checked pids; `71f2cc3` sandbox-aware liveness, warm registry names, `release --all` partial results; `5f0fb3a` verified final identity, bounded live replies; `c060686` holds the v0.2 site copy (`39aff85`) out of this PR. The code agent's own run at the head: gate clean, 2236 tests pass; my re-verification is next. Docs updater running (design §A13/§A17/§A18/§A19, json.md `release`, README, CHANGELOG, AGENTS, `.claude/` index drifts, release skill). Left: my gate and full suite, AC-602 flake-proof re-dispatch on the final head, AC-604 acceptance re-run, PR, CI, merge. |
| W7 | Pending (release v0.2.0): cherry-pick the held site copy `39aff85` and bump `site/src/data/site.ts` with Cargo.toml in the release PR. |
| agent-os port (PR #2) | Open, not merged; waiting on the owner. |

Progress against the goal: W0–W5 merged; W6 close to its PR; W7 is the release (about 88%).

## Landed on `main` this window

None.

## Decisions

- **Site copy held out of the W6 PR.** `site-deploy` runs `wrangler deploy` on every push to `main` when `CLOUDFLARE_API_TOKEN` is set, and the repo secret exists, so merging the v0.2 copy would publish v0.2 commands under a "v0.1.1" chip before v0.2.0 exists. Reverted in-branch (`c060686`); the release PR merge is the deploy.
- **S-07 labelling left open:** `glen inbox` and the watch overlay still label a null-job reply `tui` (a TUI turn and a shared batch both produce null); a conformance test pins it. Owner decision, not changed.
- **Trade-off accepted:** `glen models` runs with `--no-extensions`, so it no longer lists providers that an extension registers.
- Deferred security items stay recorded as tech debt (warm adoption secret, namespace-aware identity token, registry socket and process-image binding, worker.log bound, reply-ledger idempotency, inbox cursor CAS, admission control, `__acp` allow opt-in). The S-01 approval-mode question and the sandboxed-agent threat model need the owner.

## Harness friction

- The site agent's `--channel` literal in a site test tripped the conformance scan that bans that string; a code agent noticed it from a cross-agent test run.
- Install scripts were disabled in CI (`--ignore-scripts`); the site build passed in a scratch copy on Node 26 only, so the first Node 24.22.1 CI run is the real check.
- Implementer commits sometimes carry the Opus trailer; squash messages correct it.
- Carried for batch 13: omp deadline and heading tolerance; stale `glen` binary fallback; `cargo` not on PATH; usage limits idle the run silently; checkpoint commits need a fresh branch after a merge; chained commands must not push after a failed test step; cargo from Python on macOS 27 needs `SDKROOT` unset; `sd` not on PATH, `sed -i` and `cat` blocked by hooks.

## Blockers

None. Owner-only: rotate the temporary Cloudflare token and the R2 keys; the S-01 decision and threat-model question; the Codex MCP leg and interactive Channels check (AC-505c, d); whether to relabel null-job replies. Open for the owner: whether glen's routing moves from Codex to omp; the upstream-to-agent-os step; the agent-os port session's questions.

## Next hour

1. W6: docs land; my gate and full suite on a quiet machine; flake-proof and acceptance re-runs; PR, CI, memory, merge, closure.
2. W7: release v0.2.0 through the release skill.
