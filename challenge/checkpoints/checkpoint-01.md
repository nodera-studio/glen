# Checkpoint 1 — first hour (00:56Z → 02:10Z, 2026-10-30)

Snapshot of the unattended `/auto-mode` run that carries glen from scaffold to a released v0.1.0.
Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0` ("glen v0.1.0 autonomous run");
board module "glen v0.1.0" `3d49fa05-8207-4ab0-88d7-21e3c8becee4` (project Glen).

## Where things stand

| Wave | Board issue | State at checkpoint |
| --- | --- | --- |
| W0 Foundation | 6e0f7c78 | Implemented and committed on `feat/v010-wave-0-foundation` (d3a232f, 44 files, 71 tests green). Dual review done; a fix pass for 14 findings is running. PR not opened yet. |
| W1 Disk reader (`ls`, `show`, `tail`) | cdeeac74 | Pending |
| W2 RPC driver (`send`, `wait`) | 0007fbcc | Pending |
| W3 Live route + reply channel | 0e472c4b | Pending |
| W4 Agent surface + acceptance runs | 1624badb | Pending |
| W5 `glen watch` TUI | 8abd1783 | Pending |
| W9 omp in agent-os (new, owner) | aec92571 | Pending; starts once the CLI is proven |
| W6 Site | b8e5cbf2 | Architecture chosen; plan next. Implementation after W0 merges. |
| W7 Deploy + release workflow | 378cb31f | Pending |
| W8 Audits + v0.1.0 | 054b1fbc | Pending |

## What happened in hour 1

**Context and ground.** Read the omp, bootstrap and harness memories and the three owner Ideas in full.
No decisions or tech debt existed yet. Checks passed: `main` clean, omp 18.14.0 (same as the
verified surfaces, so no re-verification), Codex CLI 0.166.0, `gh` and `wrangler` authenticated.
Missing tools: `lefthook`, `cargo-audit`.

**Research saved to Mnemosyne (project Glen).**
- Rust CLI/TUI reference study `d81d0973` (codex-rs, heartwood, stashbase-cli, monocode, omp TUI).
- Design language study `93cec9de` rev 2. It was redone from the **Nodex redesign**
  (`apps/mockup-nodex`) after the owner's correction; rev 1 used Nodex production.
- W0 crates and the omp-rpc git dependency `6bd1c4c8`. The git dependency works, but cargo does a full
  ~1.5 GB clone, so CI caches `~/.cargo/git`.
- W6 site stack `86ef9e5c` (Astro 7.4, morphin CLI on the free registry, Workers static assets).

**Decisions taken on the owner's behalf (Mnemosyne memories, project glen).**
- `47efa1c2` **Live route = a bundled glen omp extension**, not the collab relay. Other parts of the
  decision:
  - Every `glen send` runs in a per-job detached worker. The send waits for a bounded time, then
    returns a job handle.
  - `GLEN_MANAGED=1` keeps a single `glen_reply` provider per process.
  - Liveness is probed through the omp owner lease: on Linux an abstract datagram socket named from
    XXH64, on macOS a non-blocking flock.
  - A residual fork race remains and is documented honestly.
  The Codex advisor endorsed it with changes; it found the duplicate-tool blocker and the
  `deliverAs` correction.
- `7cef5219` Crate choices: clap 4.6, ratatui 0.30 with crossterm 0.29, anstyle/anstream,
  comfy-table 8, indicatif, jiff, rustix, and omp-rpc as a git dependency pinned to v18.14.0.
- `f6a568ef` The visual reference is the Nodex redesign, not production (owner instruction).
- `6cd36a6d` Site architecture: a static-first Astro hero with a CSS terminal compiled at build
  time, morphin islands below the fold, and `src/render/palette.json` shared by the CLI and the site.
- `f667e00a` Owner scope addition: wave W9 (omp in agent-os) and these hourly checkpoints.

**How the harness was used.**
- `/orchestrate` was on. Context came from two `/explore` agents and two `plan-researcher` agents.
- W0 went through `/plan-implementation`: Claude and Codex planners, then a judge. The judge kept the
  Claude plan, grafted six Codex items, and caught a wrong source claim from Codex about the lease
  path.
- W0 was then built with `/implement`: the Codex Implementer, then `/code-review` with a Claude and a
  Codex pass, then a fix pass by a fresh Codex Implementer.
- W6 went through dual architects and a judge.

**Harness friction so far** (full list in `.claude/auto-mode/state.md`).
- The Mnemosyne board has no issue keys.
- `queue_item_add` with a board-issue `documentId` errors but still persists a broken row, which
  breaks the queue page. The queue had to be recreated.
- `codex:codex-rescue` mistakes a literal `--resume` in the brief for its own flag.
- `plan-architect` has no Mnemosyne tools.
- A Codex planner's output reaches the judge only by extracting it from the agent transcript.
- Stale "agents never commit" text sits in the agents README.
- Plan Step 7 has no auto-mode exemption.

## Next hour

Finish the W0 fixes, run local CI, open the W0 PR, wait for CI, then merge. Plan W6 in detail. Start W1
(the disk reader) right after W0 merges.
