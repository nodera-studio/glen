# Checkpoint 4 — hour 4 (04:01Z → 05:01Z, 2026-10-30)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.

## Wave ledger

| Wave | State |
| --- | --- |
| W0 Foundation | Merged, PR #3 |
| W1 Disk reader | **Merged, PR #9 → `25f8762`.** Board done + `shipped:main`. Path: three fix cycles, an 84-test conformance suite (193 total), a scoped re-review, then 371 tests green. |
| W2 RPC driver (`send`, `wait`, `jobs`) | Implementer `523568e`. **Live smoke against real omp 18.14.0 passed 8/9 steps**; step 5 is the idle-TUI limitation, now documented. Review gate: Claude found 1 high, 3 medium, 8 low; Codex found 1 P1 and 4 P2. Fix cycle 1 (15 items) merged into the wave branch. Conformance stands at 266/266, plus 8 spec ambiguities settled (`06ca213`). In flight: AC-207 re-derivation, and fix cycle 2 to match the spec clarifications. |
| W3 Live route + extension | Explorer map done. **Dual planners** running; plans will be anonymised and judged blind. |
| W3b Session controls | Pending. W3's socket protocol reserves its ops. |
| W4 / W5 / W9 / W7 / W8 | Pending |
| W6 Site | **Held for a redesign**, after the owner judged it "meh". A design survey wrote `.claude/plans/v010-wave-6-polish.md`. It rates all 41 morphin components and picks 6, inventories the Nodex redesign and marketing site, and specs a two-pane realistic terminal player. Per the owner, site code is now written by an **Opus** agent, which is running. PR #7 (morphin + main merge) stays open. |

## Landed on `main` this hour

- `7e9619e`, PR #8: omp spawn now retries on ETXTBSY. This fixes the Linux parallel-fork test race (memory `260a2ac7`).
- `25f8762`, PR #9: **W1** `glen ls`, `show`, `tail -f` (memory `e8cdbe82`).

## Decisions and records (Mnemosyne, project glen)

- `80ff9cce`, confirmed omp fact: a TUI that has opened a session but not written to it holds no owner lease. W3's extension endpoint closes this gap.
- `4d3d3eb3`, tech debt: `tail -f` re-emits a v1 session after it is rewritten.
- `e6d65010`: the morphin registry serves a 404 for the shared `utils.ts` of four components; the GitHub fallback works.
- W2 decisions:
  - **AC-218 amendment:** a startup thread plus an `abandon` flag lets abort land within 1 s while omp is still starting.
  - **The pre-prompt lease probe is dropped,** because omp's startup recovery writes take the lease first. The pre-spawn probe, identity checks and fork detection remain.
  - **8 conformance ambiguities settled** in `docs/design.md`.
- W6 decisions:
  - **Site code goes to Opus** (owner); local feedback memory `site-code-by-opus`.
  - **The inline-script budget may rise to 9 KiB** to fit the realistic terminal.
  - **esbuild 0.28.2 is pinned as a devDependency.**
- Notes: checkpoint 3 `5d5e9ed8`, W2 plan `633e767b`.

## Harness friction this hour

- **Owner judged the site "meh".** No step in `/implement` checks fidelity to the named design reference, and no step surveys the component catalogue before picking components.
- **UI code routing.** The owner routed UI code to Opus; the harness hard-codes Codex as the only author of product code.
- **TaskStop does not stop the Codex job behind an agent.** Its processes kept editing the worktree and had to be killed by cwd.
- **codex-rescue returned before finishing.** It handed back a background task id, and the orchestrator had to poll for the plan file.
- **A spec assumed a tool without checking.** It said esbuild "resolved by Vite", but Vite 8 uses Rolldown.
- **Plan contradiction found only by the Implementer:** W2's AC-218 (abort within 1 s) against blocking startup calls.
- **`.claude/plans` is git-ignored,** so worktrees don't contain the plan.
- **My own live-check polish item contradicted the plan's layout.** Conformance caught it.
- **Linux-only cfg imports are not linted on the macOS box.** This caused W1 CI's red run.

All of these are logged `[open]` in the state file and queued for harness batch 5.

## Blockers

None.

## Next hour

- **W2:** finish conformance and fix cycle 2, run a scoped re-review, rerun the live smoke steps that changed, then PR, merge and re-index.
- **W3:** blind judge, plan Note, network pre-flight if one is needed, then the Implementer (Codex for the Rust and TS crate code).
- **W6:** finish the Opus polish, then a visual and fidelity check against the Nodex references and Lighthouse, then merge PR #7.
- **Harness batch 5:** UI routing, design-fidelity gate, Codex cancel path, codex-rescue foreground completion, plan paths in worktrees, Linux clippy target in the gate, dependency verification in specs.
