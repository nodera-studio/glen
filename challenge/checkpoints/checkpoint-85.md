# Checkpoint 85 — 03:20Z → 04:20Z, 2026-11-04 (v0.2.0 run, hour 51)

Run ledger: Mnemosyne queue `b25ebce7-b0f0-46b3-839b-f4721a31e920`; state file `.claude/auto-mode/state.md` (Status: in-progress). Board module "glen v0.2.0" `040f2580-aedf-422f-9ef9-cfd24435e8e2`.

## Wave ledger

| Wave | State |
| --- | --- |
| Plan | Done. |
| v0.2 W0–W5 | Merged (PR #35 `35b7522`, #36 `715b1ff`, #37 `0157285`, #38 `affb069`, #39 `d1708f0`, #40 `582f77b`). |
| v0.2 W6 audits (`0ae5e983`) | **PR #41 open; AC-602 still open.** Root cause of the "binary missing mid-run" class found and fixed in `25a812f`: on macOS cargo copies (not hard-links, cargo#10060) uplifted binaries and removes then re-copies them on every build, fresh or not, so any test running `cargo build`/`cargo test --no-run` mid-suite made `glen` or `fake_omp` vanish briefly for concurrent spawns. Fix: `flake::example`/`flake::cargo`/`flake::glen` take a cross-process lock and publish binaries into `target/<profile>/glen-test-bin/` by hard link + rename; tests exec from there. Stress before/after 5/500 → 0/500; loaded conformance 5/5. My gate and suite on `25a812f`: 2240 passed, 0 failed. PR CI green on all checks (incl. `linux-sandbox` and macOS). flake-proof run 41770521798 in progress on both OSes (previous run 41761493795 cancelled: pre-fix head). Left: flake-proof green, merge memory (own call), merge, closure. |
| W7 | Pending (release v0.2.0). |
| Off-wave: PR #42 | Header hairline fix; CI green; awaiting the owner on the `.exit-badge` icon-pop scope, then merge. |
| agent-os port (PR #2) | Open; waits on the owner. |

Progress against the goal: about 95%.

## Landed on `main` this window

None.

## Decisions

- Test code that builds or runs a cargo artifact goes through the `flake` helpers and execs from `glen-test-bin/` (Mnemosyne `bf3e4278`).

## Harness friction

- AGENTS.md names `scripts/suite-lock.sh`; it lives at `.claude/scripts/suite-lock.sh` (docs drift, fix in the release PR).
- Carried for batch 13: usage limits idle the run silently; conformance writers must scale every upper bound through the flake helper; `gh api .../logs` needs `--allow-escape-sequences`.

## Blockers

None for the agents. Owner: PR #42 scope, plus the standing owner-only items.

## Next hour

1. W6: watch flake-proof 41770521798; on green, merge memory, merge PR #41, closure.
2. W7: release v0.2.0.
