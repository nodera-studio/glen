# Checkpoint 89 — closing, 07:20Z → 08:05Z, 2026-11-04 (v0.2.0 run, hour 55)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920` (all 9 items done); board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2` (done).

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0–W6 | Merged (PR #35 `35b7522`, #36 `715b1ff`, #37 `0157285`, #38 `affb069`, #39 `d1708f0`, #40 `582f77b`, #41 `7ed1c82`). |
| v0.2 W7 release | **Done.** Release PR #43 merged as `c7fb6ac`. Annotated tag `v0.2.0` on it. Release workflow run 41795557743 green with 9 assets. Install smoke: `glen 0.2.0` (aarch64-apple-darwin, checksum OK). Site live: chip `v0.2.0`, "works with omp 18.14.0 to 18.16.0". Acceptance on final main (AC-v2-604): mode p 8/8 (`wake=woke`), mode m 7/7. Release memory `54ed9018`. |
| Off-wave: PR #42 | Header hairline fix; CI green; open, awaiting the owner on the `.exit-badge` icon-pop scope. |

Progress against the goal: 100%. The goal's finish line (annotated `v0.2.0` tag via the release skill) is met.

## Landed on `main` this window

- `c7fb6ac` chore: release v0.2.0 (#43).
- Tag `v0.2.0`.

## Decisions

- `max_tested` 18.16.0 (nightly omp-latest passed on it); `min` and `tested` stay 18.14.0.
- The skill pre-approves only `glen ls`, `show`, `tail`, `jobs`, `wait`, `inbox`, `doctor`, `send`; omp's default approval mode is unchanged.
- The CHANGELOG names `__acp` once, as hidden and unsupported (AC-v2-304 requires the mention).

## Closing

- Board: all eight wave issues `done`, `shipped:main`, `released:v0.2.0`; module `done`.
- Queue: all items done.
- Tech debt: one active memory found (`e72263da`, new this run, not fixed); W6 deferred hardening recorded as `0636e7a1`.
- Worktrees: the release worktree removed; `agent-a17ef3d…` kept for PR #42; `upbeat-hermann-da1c04` is not this run's.
- Scratch: this run's 20 `glen-w1-*` and `glen-impl-snap-*` entries removed; older `glen-w12-*` and `glen-links-*` (v0.1 run, ~4.5 GB) and `glen-omp-engine*` left for the owner.
- Branches: every merged run branch deleted on origin.
- This PR carries the docs and tech-debt audit reports (gitleaks clean); the security audit stays local and gitignored (public repo).
- After this PR merges: the hourly checkpoint schedule is deleted and the state file set to `done`.

## Owner-pending

- Rotate the temporary Cloudflare token and the R2 keys.
- Real-omp acceptance legs (a, b, b3, c1), the Codex legs (c2, mode m Codex) and the interactive Channels check.
- S-01 follow-ups and the sandboxed-agent threat model.
- Whether to relabel null-job replies in the TUI.
- PR #42 icon-pop scope; agent-os port PR #2.

## Harness friction (for batch 13)

- Usage limits idle the run silently (about nine hours this run).
- Flake fixes scoped to the test CI named cost about seven CI rounds; sweep the whole tree first.
- macOS cargo re-copies uplifted binaries on every build; test code must exec from `glen-test-bin/`.
- Conformance writers must scale every upper bound through the flake helper.
- `gh api .../logs` needs `--allow-escape-sequences`.
