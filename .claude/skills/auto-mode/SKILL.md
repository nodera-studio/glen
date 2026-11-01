---
name: auto-mode
description: >
  glen's autonomous wave-by-wave implementation loop. Drives a declared goal (a Glen
  board issue or module on the Mnemosyne workspace, a plan doc, or a freeform objective)
  end to end WITHOUT a user gate between waves: derive waves → /plan-implementation →
  create and flip board issues → /implement (with its Claude ‖ Codex ‖ omp review gate) →
  open a PR into `main` → poll CI → fix every finding → store a Mnemosyne memory →
  squash-merge → next wave; cut a `v0.x.y` release tag only when the goal asks for one.

  Use this skill whenever the user runs /auto-mode or /goal, or asks to "ship this board
  issue autonomously", "run the loop until <goal>", "keep implementing waves until done",
  "auto-pilot this issue", "work through all the sub-issues by yourself", or otherwise
  asks Claude to carry a multi-wave feature to merged completion without checking in
  between waves. Trigger even when the user does not say "auto-mode" but describes an
  unattended multi-wave implementation marathon with a clear finish line.
argument-hint: '<goal — Glen board issue/module, plan doc path, or freeform objective> [and release v0.x.y]'
---

# Auto-Mode — Autonomous Wave Loop (glen)

## Role

You are the orchestrator of an unattended, multi-wave implementation marathon. The
user has handed you a finish line and walked away. Reach it — every wave planned,
implemented, reviewed, green, and squash-merged onto `main` (and tagged as a release when
the goal asks for one) — surfacing the user only for things that genuinely require a
human, and only after you have exhausted your own ability to decide.

Before Wave 1, recall Mnemosyne project `glen` (`memory_search` on the goal's topic,
`memory_list` with `sourceKind: "decision"`) and follow the decisions you find. The
detailed recipes (state-file template, wave derivation, board operations, per-step
expansions, polling, the escalation ladder, blocker criteria, reporting cadence) live in
[`references/loop-procedures.md`](./references/loop-procedures.md) — read it before the
first wave and whenever you need the long form.

## The goal contract (shared state file)

`/auto-mode` and `/goal` cooperate through one state file: `.claude/auto-mode/state.md`
— the single source of truth so the loop survives context compaction. It is git-ignored;
never stage it. (Template in the references file.)

- `/goal <objective>` writes/overwrites the **Goal** section, sets status `goal-set`,
  confirms in one line, and stops. It declares intent only — never starts the loop.
- `/auto-mode [args]`: read the state file; if args carry an inline goal persist it as
  a `/goal` write first; if no goal is set, ask for one and stop (a loop with no finish
  line is the one thing not to start); if a goal exists, resume or begin the loop.

## Branch model — `main` is the only trunk

glen has one long-lived branch, `main` (see the `branching-model` skill). There is no
staging branch, no integration branch and no promotion step.

- **Every wave** branches off `main` as `feat/<goal-slug>-wave-<N>-<slug>`, PRs into
  `main`, and squash-merges when the bar is met. The feature accumulates on `main` wave
  by wave, so **every wave must leave `main` green and usable**: an incomplete
  subcommand stays unregistered or hidden (`#[command(hide = true)]`) until the wave that
  finishes it.
- **Release (only when the goal asks for one).** A goal that says "release", "tag
  v0.x.y", "ship a version" or names a version ends with the `release` skill: a
  version-bump PR into `main`, then an annotated `v0.x.y` tag on the merged commit. A
  goal that does not ask for a release never bumps the version or tags.

## Resolving the goal into waves

**Board children first, plan fallback.** The tracker is the Mnemosyne workspace board of
project **Glen** (`projectId` `ff92abe1-da63-402d-a4f1-c8690e471be1`), not Linear and not
GitHub Issues. A goal naming a board module or issue with children → each child issue is
a wave (`board-children`); no children but a matching `.claude/plans/` wave breakdown →
use it (`plan-doc`); neither → run `/plan-implementation`, derive waves, then create one
module for the goal and one issue per wave under it (`fresh-plan`). Write the full wave
ledger before Wave 1 so scope survives compaction. (Board calls in the references file.)

## The per-wave pipeline (no user gate between waves)

Per wave, in order — this sequence is the spine of the mode:

1. **Bootstrap** — `git fetch origin && git switch main && git pull --ff-only`, branch
   `feat/<goal-slug>-wave-<N>-<slug>`, flip the wave's board issue `backlog`/`todo` →
   `in_progress`.
2. **Plan if needed** — `/plan-implementation` scoped to the wave (skip if a plan
   already exists).
3. **Implement** — invoke `/implement`; let it own its internal gates, including the
   local `/code-review` gate (Claude ‖ Codex, + omp when its probe is ready). That gate is the review: the GitHub repo
   has no review bots and no branch protection.
4. **Pre-push check** — `cargo fmt --all -- --check && cargo clippy --all-targets
   --locked -- -D warnings && cargo test --locked` (or `/run-ci`); fix red before
   pushing. The lefthook pre-push hook runs clippy and the tests too, when installed.
5. **Open the PR into `main`** (`gh pr create --base main`), naming the board issue (`W<n>` label or document id)
   in the title or body; flip the issue → `in_review`.
6. **Poll** — `gh pr checks <N> --watch` (blocks until CI settles), or wake ~270s apart
   with ScheduleWakeup; each cycle also fetch new PR comments and reviews since the poll
   cursor; update the cursor. Idling over ~10 min on background agents, a workflow or CI →
   a `ScheduleWakeup` heartbeat about every 20 min; a background agent that owns a PR's CI
   loop gets a `.claude/auto-mode/ci-owner/pr-<N>.json` marker, and you leave that PR's CI
   events to it (references → step 6). On every wake, `bash .claude/scripts/checkpoint-due.sh`
   first when the goal has checkpoints; overdue → write the checkpoint before anything else
   (references → Run checkpoints).
7. **Triage + fix** — start every fix cycle (CI, review or conformance) by merging main
   into the wave checkout: `git -C "$WT" fetch origin && git -C "$WT" merge --no-edit
   origin/main`, never a rebase (a CI-owner agent merges in its own checkout; you merge only
   into a clean `$WT`); a conflict goes to `routing.sh integration`; after a
   non-trivial merge re-run `quality-gate.sh` and `cargo test --locked`, and name the merged
   SHA in the fix brief (references → step 7). A branch cut from another wave's pre-squash
   commit uses the squash-merged base recipe (`git replace --graft`, references → step 7);
   fix-cycle decisions quote their governing AC (`Governing AC: AC-<id> — "<verbatim>"`).
   Then CI red or a comment with a real defect → fix, push, poll again; keep tests + docs in
   sync.
8. **Capture, then merge when the bar is met** — the `ci` checks green on both runners
   (`check (ubuntu-latest)`, `check (macos-latest)`: fmt, clippy `-D warnings`, test),
   no unresolved review comments, the PR mergeable. THEN **store the wave's durable
   decisions/learnings in Mnemosyne referencing the PR** (`memory_store`,
   `project: "glen"`, `PR #<N>` in the title, content or tags; the `capture-before-merge`
   hook blocks `gh pr merge` until you do) → `gh pr merge <N> --squash`. Don't pass
   `--delete-branch`; delete the branch at wave close.
9. **Close the wave** — flip the board issue → `done`, add the tag `shipped:main`, post
   the wave-end comment (files / commit / PR / tests), write the wave-end Note, mark the
   wave's queue item `done`, refresh the code-index clone and `code_reindex`, update the
   ledger, then delete the wave branch (`git push origin --delete <branch>`). The harness
   self-improvement batch and any independent wave run in their own worktrees alongside
   (references → Parallel work).
10. **Advance** to the next wave without asking. 2–3 line status at boundaries only;
    silent during polling.
11. **Release only if the goal asks for one** — when every wave is merged: status
    `landing`, run the `release` skill (version-bump PR → green CI → capture → squash-merge
    → annotated `v0.x.y` tag on that `main` commit), then tag every goal issue
    `released:v0.x.y`. A goal without a release is `done` once its last wave is merged and
    tagged `shipped:main`. Either way, then "Closing the run" (references), which sets status
    `done` at its end.

## Hard rules

- **Base = `main`; one PR per wave straight into it**, squash-merged. No staging, no
  integration branch, no direct pushes to `main`, no force-push to a shared branch,
  never `--no-verify`.
- **Each wave leaves `main` green and usable.** Incomplete surfaces stay unregistered or
  hidden until finished.
- **Capture before every merge** — `memory_store` with `project: "glen"` referencing the
  PR, before `gh pr merge`. The merge keeps the code; the memory keeps the reasoning.
- **No user gates between waves** — that autonomy is the point. Decide for yourself via
  the escalation ladder (read codebase → probe the tools you have → dispatch a
  specialist → only then flag); flag only true human-required blockers (credentials,
  material scope trade-offs, external approvals, a wave red 3+ cycles with no path).
- **Board flips**: `in_progress` at wave start (step 1), `in_review` at PR open (step 5),
  `done` + tag `shipped:main` at wave close (step 9); `released:v0.x.y` only after the
  release tag is pushed (step 11). Tags are added with `document_tags_update`
  (`mode: "add"`), which leaves existing tags alone.
- **glen's invariants hold in every wave**: never write to an omp session `.jsonl`; tests
  use `tests/fixtures/`, never a live `~/.omp`; never edit `../metis/references/oh-my-pi`.
- **Confirm a routing change before broadcasting it.** Restate an owner routing directive
  in chat as one line ("Routing: implementation → claude (Codex keeps
  review/plan/debug/audit). Apply and tell N running agents?") and apply it only after the
  owner confirms. Then store the decision memory, write `.claude/auto-mode/routing.json`
  with `decision` set to its id, and only then SendMessage the running agents, pointing
  them at `bash .claude/scripts/routing.sh` instead of restating the rule. Why: e5464a64
  was broadcast and reversed minutes later, then superseded by 148655d9.
- **Revise briefs in the file.** A changed or revoked decision goes into the brief or plan
  file as `REVISED`/`REVOKED` before any message; committers re-read it before committing
  (references → Harness self-improvement loop).
- **Commit attribution**: follow the attribution lines the session provides; never invent
  a different trailer.

## Stop conditions

- Goal without a release: last wave merged + issue `done` with `shipped:main`. Release
  goal: tag pushed + issues tagged `released:v0.x.y`. Either way, then "Closing the run"
  (references), which sets status `done`; final summary.
- A wave PR red 3+ cycles with no path forward → stop that wave, flag the user.
- User says stop/pause or changes the goal → stop, confirm current state.

## Cross-references

- [`references/loop-procedures.md`](./references/loop-procedures.md) — state-file
  template, wave derivation, Mnemosyne board calls, per-step expansions, polling,
  escalation ladder, blocker criteria, reporting cadence.
- `.claude/commands/implement.md`, `.claude/commands/plan-implementation.md` — the
  per-wave engines this loop drives; `.claude/commands/code-review.md` — the review gate.
- `.claude/skills/branching-model/SKILL.md` — trunk-only rules; `.claude/skills/release/SKILL.md`
  — the release runbook.
- `.claude/CLAUDE.md` — glen's profile, Mnemosyne project and board ids, tool routing.
- `.claude/rules/` — path-scoped conventions; consult before deciding "unclear".
