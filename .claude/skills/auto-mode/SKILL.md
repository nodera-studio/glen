---
name: auto-mode
description: >
  Autonomous wave-by-wave implementation loop. Drives a declared
  goal (a ticket from your issue tracker, a plan doc, or a freeform objective) end to end
  WITHOUT a user gate between waves: derive waves → /plan-implementation → create+flip
  tracker tickets → /implement → capture a durable memory (if you run one) → open PR into
  your staging trunk → poll CI + code-review bots → fix every finding → merge → next wave;
  promote staging → release with a merge commit only when the goal targets prod.

  Use this skill whenever the user runs /auto-mode or /goal, or asks to "ship
  {{TICKET_PREFIX}}-XXX autonomously", "run the loop until <goal> on the main/staging
  branch", "keep implementing waves until done", "auto-pilot this ticket", "work through
  all the sub-issues by yourself", or otherwise asks Claude to carry a multi-wave feature
  to merged completion without checking in between waves. Trigger even when the user does
  not say "auto-mode" but describes an unattended multi-wave implementation marathon with
  a clear finish line.
argument-hint: '<goal — tracker ticket, plan doc path, or freeform objective> [--base main|staging]'
---

# Auto-Mode — Autonomous Wave Loop

## Role

You are the orchestrator of an unattended, multi-wave implementation marathon. The
user has handed you a finish line and walked away. Reach it — every wave planned,
implemented, reviewed, green, and merged onto your staging trunk (and promoted to your
release branch when the goal targets prod) — surfacing the user only for things that
genuinely require a human, and only after you have exhausted your own ability to decide.

If you run a durable-memory system (see the `mnemosyne` skill), consider formalizing the
canonical per-wave pipeline (comment polling, triage, merge criteria) as a named memory —
e.g. `feedback-autonomous-multi-wave-loop` — and treat its per-wave details as
authoritative once written; don't contradict it. The detailed recipes (state-file
template, wave-derivation, per-step expansions, shipping-label mechanics, code-review-bot
command hooks, the escalation ladder, blocker criteria, reporting cadence) live in
[`references/loop-procedures.md`](./references/loop-procedures.md) — read it when you need
the long form.

## The goal contract (shared state file)

`/auto-mode` and `/goal` cooperate through one state file: `.claude/auto-mode/state.md`
— the single source of truth so the loop survives context compaction. (Template in
the references file.)

- `/goal <objective>` writes/overwrites the **Goal** section, sets status `goal-set`,
  confirms in one line, and stops. It declares intent only — never starts the loop.
- `/auto-mode [args]`: read the state file; if args carry an inline goal persist it as
  a `/goal` write first; if no goal is set, ask for one and stop (a loop with no finish
  line is the one thing not to start); if a goal exists, resume or begin the loop.

## Branch scope — staging is the trunk

The default base is your **staging trunk** (e.g. `staging`), and the loop PRs each wave
**directly into it** — there is no per-goal integration branch (see the
`branching-model` skill). The **release branch** (e.g. `main`) is release-only: it is
reached solely by a staging → release **promotion** (a merge commit, never a squash), so
it never originates work and never drifts from staging.

A goal resolves to one of two shapes:

- **Feature goal (default, base = staging trunk).** Each wave branches off the staging
  trunk, PRs into it, and merges (squash) when green. The feature accumulates on staging
  wave by wave. Because half-built work now lives on the shared preview branch, **every
  wave must leave staging green and non-broken** — gate incomplete surfaces behind a
  flag, don't wire a half-finished route. A feature that cannot ship incrementally still
  lands in staging-safe waves — keep the incomplete surface flag-gated until the final
  wave wires it up. There is no integration-branch exception; that is the branch shape
  this model retires.
- **Promotion goal (target the release branch).** "promote staging → main" / "ship the
  staging batch to prod" → open ONE PR staging → release branch, gate it on green CI and
  your CI-guard workflow (if configured), and merge with **`gh pr merge --merge`** (merge
  commit). Never squash/rebase a promotion — that re-creates release↔staging drift.

Parse the base from the goal phrasing — explicit promotion intent only (`to prod`,
`promote staging to main`, `on the main branch`) routes to a promotion; a bare/ambiguous
`on main` should be confirmed with the user, not assumed. Everything else, and anything
unstated, defaults to the staging trunk — the safe, reversible base.

## Resolving the goal into waves

**Tracker sub-issues first, plan fallback:** a ticket with children → each child is a
wave (`tracker-subissues`); no children but a matching `.claude/plans/` wave breakdown →
use it (`plan-doc`); neither → run `/plan-implementation`, derive waves, create one
tracker sub-issue per wave (`fresh-plan`). Write the full wave ledger before Wave 1 so
scope survives compaction. (Detail in the references file.)

## The per-wave pipeline (no user gate between waves)

Per wave, in order — this sequence is the spine of the mode:

1. **Bootstrap** — `git fetch`, `git checkout <staging-trunk> && git pull`, branch
   `feat/<feature>-wave-{N}-{slug}` off the staging trunk, flip the wave's tracker
   ticket(s) → `In Progress`.
2. **Plan if needed** — `/plan-implementation` scoped to the wave (skip if a plan
   already exists).
3. **Implement** — invoke `/implement`; let it own its internal gates.
4. **Pre-push check** — run your local CI-check command (e.g. `make ci-check`); fix red
   before pushing.
5. **Open the PR into the staging trunk** (`gh pr create --base <staging-trunk>`).
6. **Poll continuously** — wake ~270s apart; each cycle `gh pr checks` + fetch new
   comments/reviews since the poll cursor; update the cursor.
7. **Triage + fix** — Critical/Major → fix+push+reschedule; Minor → fix or batch;
   keep tests + docs in sync.
8. **Capture, then merge when the bar is met** — all checks green (Lint+Typecheck,
   Build, Unit+Integration, E2E, and any configured code-review bots), no new comments
   since the last push, and ≥1 clean cycle (zero new comments AND green CI) to defend
   the bot-posts-after-CI-greens race. THEN, **before merging, store the wave's durable
   decisions/learnings referencing the PR** if you run a durable-memory system (a
   pre-merge hook can enforce this) → `gh pr merge {N} --squash` into the staging trunk.
   Do NOT pass `--delete-branch` if you want deletion to stay a deliberate post-capture
   step; delete the branch yourself at wave close.
9. **Close the wave** — flip the tracker ticket → `Done`, post the wave-end comment
   (files / commit / PR / tests), update the ledger, apply a **`shipped:staging`**-style
   label (the code is now on the staging trunk), then delete the wave branch.
10. **Advance** to the next wave without asking. 2–3 line status at boundaries only;
    silent during polling.
11. **Promote to prod only if the goal targets the release branch** — when every wave is
    on the staging trunk: status `landing`, open ONE PR staging → release branch, poll
    the same way, merge with **`gh pr merge --merge`** (merge commit, never squash), then
    flip every goal ticket's shipping label from staging to prod and set status `done`. A
    staging-only goal is `done` once its last wave is merged + shipped-to-staging.

## Hard rules

- **Base = staging trunk; one PR per wave straight into it** — no integration
  branch, no exception. A feature that can't ship incrementally still lands in
  staging-safe waves with the incomplete surface flag-gated. Each wave must leave
  staging green and non-broken.
- **The release branch is promotion-only, via a merge commit.** Only a staging →
  release-branch PR touches it, gated on your CI-guard workflow (if configured) + green
  CI, merged with `gh pr merge --merge` (never squash/rebase — that drifts the release
  branch from staging).
- **Capture before every merge** if you run a durable-memory system — store the PR's
  durable decisions/learnings before `gh pr merge` (a pre-merge hook can enforce this) —
  the merge preserves the code, but the knowledge is lost unless you write it.
- **No user gates between waves** — that autonomy is the point. Decide for yourself via
  the escalation ladder (read codebase → probe the tools you have → dispatch a
  specialist → only then flag); flag only true human-required blockers (credentials,
  material scope trade-offs, external approvals, a wave red 3+ cycles with no path).
- **Tracker flips**: `In Progress` at wave start (step 1), `Done` + `shipped:staging` at
  wave close (step 9); `shipped:prod` ONLY after the staging → release-branch promotion
  merges (step 11). Most trackers' label-set APIs REPLACE the whole set on write, so
  always read current labels first and pass the full desired set. (Label mechanics +
  promotion detail in the references file.)
- **No `Co-Authored-By` trailer on commits** if your project convention says not to add
  one — confirm your project's commit-attribution convention first.

## Stop conditions

- Staging-only goal: last wave merged + shipped-to-staging → status `done`. Promotion
  goal: staging → release-branch merged + tickets flipped shipped-to-prod → status
  `done`. Final summary either way.
- A wave PR red 3+ cycles with no path forward → stop that wave, flag the user.
- User says stop/pause or changes the goal → stop, confirm current state.

## Cross-references

- If you run a durable-memory system, consider a canonical per-wave-pipeline memory (see
  above) and a workflow-conventions memory for how/when your tracker's fields flip, and
  a commit-attribution convention memory — authoritative once written; this skill layers
  on top.
- [`references/loop-procedures.md`](./references/loop-procedures.md) — state-file
  template, wave-derivation, per-step expansions, shipping-label mechanics, code-review
  bot command hooks, escalation ladder, blocker criteria, reporting cadence.
- `.claude/commands/implement.md`, `.claude/commands/plan-implementation.md` — the
  per-wave engines this loop drives.
- Your project's branching-model doc — the staging-trunk / release-branch model, the
  two no-drift rules, the merge-method contract, and the enforcement stack.
- `.claude/rules/` — path-scoped conventions; consult before deciding "unclear".
