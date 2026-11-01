# Auto-Mode — Detailed Loop Procedures (glen)

> Detailed sub-procedures for `.claude/skills/auto-mode/SKILL.md`. The SKILL.md keeps the
> goal contract, the per-wave pipeline sequence, entry/exit conditions, and the hard
> rules. This file holds the long-form recipes: state-file template, wave derivation,
> the Mnemosyne board calls, the per-wave step expansions, polling, shipping tags, the
> escalation ladder, blocker criteria, and reporting cadence.
>
> glen's fixed values: trunk `main` (the only long-lived branch); GitHub repo
> `nodera-studio/glen`; CI `.github/workflows/ci.yml` (job `check` on `ubuntu-latest` and
> `macos-latest`: `cargo fmt --all -- --check`, `cargo clippy --all-targets --locked -- -D
> warnings`, `cargo test --locked`); tracker = the Mnemosyne workspace board of project
> **Glen**, `projectId` `ff92abe1-da63-402d-a4f1-c8690e471be1`; memory project `glen`.

## State file template

Create `.claude/auto-mode/state.md` from this on first write and keep it current at
every state change — it is your durable memory across compaction. It is git-ignored;
never stage it.

```markdown
# Auto-Mode State

## Goal

<one-line objective — e.g. "Ship session discovery (`glen ls`) onto main">

## Status

<goal-set | planning | in-progress | landing | blocked | done>

## Resolved goal

- Goal module / issue: <title + document id + key, or "n/a">
- Release: <none | v0.x.y (the goal asked for a release)>
- Wave source: <board-children | plan-doc | fresh-plan>
- Shipping tags: shipped:main at each wave close; released:v0.x.y only after the tag is pushed
- Run ledger queue: <queue id> (one item per wave; mirrors the table below)

## Wave ledger

| Wave | Board issue (key / id) | Branch                       | PR → main | State                                                  |
| ---- | ---------------------- | ---------------------------- | --------- | ------------------------------------------------------ |
| 1    | <key> / <uuid>         | feat/<goal>-wave-1-<slug>    | #NN       | merged \| in-progress \| polling \| blocked \| pending |

## Harness friction log

- <date> <wave> <what in .claude/ slowed or misled the run> — <fixed in PR #NN | open>

## Harness self-improvement

| Batch | Branch                        | Worktree                              | PR → main | State |
| ----- | ----------------------------- | ------------------------------------- | --------- | ----- |
| 1     | chore/harness-selfimprove-1   | .claude/worktrees/harness-1           | #NN       | <...> |

## Release (release goals only)

- Bump PR: <#NN or "n/a — no release asked"> — <state>
- Tag: <v0.x.y pushed at <sha> | pending | n/a>

## Poll cursors

- PR #NN last-seen comment: <ISO timestamp>

## Checkpoints

- Cadence: 60m            (or `none` when the goal asks for no checkpoints)
- Last checkpoint: 2026-11-01T08:01:50Z (checkpoint-41)   (or `none` before the first)
- Path: challenge/checkpoints/checkpoint-NN.md (the path the goal's DoD names)
- Schedule: <cron job id from CronCreate> | none

## Processes started

| pid | started (ps lstart) | command | why | stopped |
| --- | --- | --- | --- | --- |

## Blockers

- <none, or the open blocker + exactly what you asked the user>

## Decision log

- <date> <decision taken autonomously + basis (research/architect/codebase ref)>
- <date> <fix-cycle decision> — Governing AC: AC-<id> "<verbatim>" | none (basis <path:line>) — <basis>
```

Times in the state file are UTC ISO from `date -u +%Y-%m-%dT%H:%M:%SZ`. Under **Checkpoints**,
`Last checkpoint: none` with a cadence is due at once, and the next checkpoint number is the
parenthesised `checkpoint-NN` plus one (`checkpoint-01` when `none`). Every process the run
starts in the background and leaves running (a preview server, a watcher) gets a
**Processes started** row with its `ps -o lstart= -p <pid>`; `stopped` is filled when it is
stopped (step 9, "Closing the run").

## The Mnemosyne board (glen's tracker)

All tools are `mcp__plugin_mnemosyne_workspace__<tool>`. Load their schemas with
ToolSearch (`+plugin_mnemosyne_workspace board document`) before the first call.

- **Shape.** A project holds modules; a module holds issues; issues nest up to three
  levels. Modules do not nest. Ideas are not the backlog.
- **Statuses** (issues and modules): `backlog` (the default on create), `todo`,
  `in_progress`, `in_review`, `done`, `cancelled`.
- **Read the board:** `board_list {projectId, moduleId?, parentId?, kind?, statuses?, query?}`
  returns items in board order, each after its parent, with `blockedBy`.
- **Read one item:** `document_get {id}` returns the item with its `revision`, place on the
  board, tags, comments and relations. Every write needs the latest `revision` as
  `expectedRevision`; on `CONFLICT`, re-read and retry.
- **Create:** `document_create {projectId, kind: "modules" | "issues", title, markdown,
  status?, priority?, tags?}`. There is no parent field on create: nest a new issue with
  `board_place {id, parentId: <module or issue id>, after?}`.
- **Graduate an idea:** if the goal is a Mnemosyne Idea, use `board_promote {id, kind,
  parentId?, title?, markdown?, status?, tags?}` instead of creating by hand; it records
  `derives-from` back to the idea and leaves the idea unchanged.
- **Change status:** `document_update {id, expectedRevision, status, ...}`. Its schema
  gives omitted fields defaults (`markdown: ""`, `tags: []`, `priority: "none"`,
  `assigneeId: null`, `checklist: []`), so pass back the item's current `title`,
  `markdown`, `tags`, `priority`, `assigneeId` and `checklist` from `document_get`
  together with the new `status`. After the first status write of the run, `document_get`
  again and confirm nothing was blanked.
- **Tags:** `document_tags_update {id, expectedRevision, mode: "add" | "remove", tags}`
  is atomic and leaves other tags alone. Use it for `shipped:main` and `released:v0.x.y`.
- **Comment:** `document_comment {id, expectedRevision, markdown}`.
- **Issue keys.** Use the key the board returns for an issue (in `document_create` /
  `document_get` / `board_list` responses) in branch slugs, PR titles and commit
  messages. The Glen prefix is not documented yet (`.claude/CLAUDE.md` → Issue tracking):
  the first issue created settles it — record it in the state file and in the decision
  log, and tell the user in the final summary so `.claude/CLAUDE.md` can be updated. If a
  response carries no key, reference the issue by title and document id.
- **Errors.** `FORBIDDEN` / `NOT_FOUND` is the token's boundary: do not work around it;
  record it as a blocker. HTTP 401 means the Mnemosyne token is missing or expired: a
  human-required blocker.

## The run ledger queue

A Mnemosyne queue is the run's visible ledger; the state file's wave ledger mirrors it and
stays the source of truth across compaction. Create one queue per goal
(`queue_create`), record its id under **Resolved goal**, and add one item per wave with
`queue_item_add {queueId, title: "W<n> — <summary> (<issue id>)", runState: "planned"}`.
**Never pass the issue id as `documentId`** (the `mnemosyne` skill → Queues: the server
persists a broken row that breaks `queue_get`); the id goes in the title. Move the item with
`queue_item_update` as the wave moves: `in_progress` at bootstrap, `blocked` when flagged,
`done` at wave close. Wave-end Notes may be referenced by `documentId`.

## Run checkpoints

When the goal's DoD asks for periodic checkpoints, fill **Checkpoints** in the state file at run
start: the cadence, the path the DoD names, and the id of the `CronCreate` job that prompts you
at that cadence (load `CronCreate` with ToolSearch first). A checkpoint is a state snapshot in
the path's format (wave ledger, what landed, decisions, harness friction, blockers, the next
hour, as in `challenge/checkpoints/checkpoint-40.md`), committed with the active branch and
mirrored as a Note. Update `Last checkpoint` when you write one.

- **On every wake** (a heartbeat, a CI poll, an agent's completion notification, the cron
  prompt), run `bash .claude/scripts/checkpoint-due.sh` before anything else:
  - exit 0 (due or overdue) → write the checkpoint it names first, then handle the event;
  - exit 1 (not due) → schedule the next wake no later than the time it prints
    (`ScheduleWakeup` delay = the smaller of about 20 min and the minutes it prints);
  - exit 3 → the goal has no checkpoints;
  - exit 2 → the **Checkpoints** section is malformed: fix it, then run the check again.
- Why: checkpoint 40 covered 00:01Z–07:38Z because nothing checked lateness while the session
  waited on long agents.
- Checkpoints continue through landing, the release and the run close until status `done`
  ("Closing the run").
- **Untracked copies block `git pull --ff-only`.** A checkpoint (or any file) left untracked in
  the main checkout blocks the pull once `origin/main` adds the same path. Before pulling in the
  main checkout, for each untracked file `origin/main` adds
  (`git diff --name-only --diff-filter=A HEAD origin/main`), compare it:
  `git diff --no-index --quiet <file> <(git show origin/main:<file>)`. Identical → move the
  untracked copy to the scratchpad, then pull. Different → stop and reconcile before pulling
  (checkpoint 38).

## Resolving the goal into waves

1. **Goal names a board item** (a module, or an issue, by key, title or id) → read it
   with `board_list {projectId, moduleId | parentId}` / `document_get`. Child issues exist
   → each open child (`backlog`/`todo`/`in_progress`) is a wave, in board order. Record
   `wave source: board-children`.
2. **No children** → look for an existing plan under `.claude/plans/` matching the goal
   with a wave breakdown. Use those waves. Record `plan-doc`. Create the board issues for
   them as in step 3 if they don't exist yet.
3. **Neither** → run `/plan-implementation`, derive waves from the plan, then:
   create one module for the goal (`document_create kind: "modules"`, unless the goal
   already names one), one issue per wave (`document_create kind: "issues"`, markdown =
   the wave's scope and acceptance criteria), and nest each under the module with
   `board_place`. Record `fresh-plan`.

Write the full wave ledger (with each issue's key and id) and the run ledger queue before
starting Wave 1.

## The per-wave loop — step expansions

Per wave, in order. The OUTER loop runs with no user gate between waves.

1. **Bootstrap.** `git fetch origin && git switch main && git pull --ff-only`, then
   `git switch -c feat/<goal-slug>-wave-<N>-<slug>`. Flip the wave's issue →
   `in_progress` (`document_get` → `document_update`).
2. **Plan if needed.** `board-children` + non-trivial wave → run `/plan-implementation`
   scoped to this wave. `plan-doc` / `fresh-plan` → the plan exists; skip re-planning.
3. **Implement.** Invoke `/implement` for the wave. It owns its gates: the cargo loop of
   the Implementer `bash .claude/scripts/routing.sh implementation` names
   (`impl-coder-claude` or `impl-coder-codex`) and `.claude/scripts/quality-gate.sh`, the local
   `/code-review` gate (1 Claude ‖ 1 Codex pass, 2-vote), the conformance tail
   (`cargo test --test conformance`) and the docs updater. This local gate is the code
   review; nothing on GitHub reviews the PR. Saved review or plan files per engine are
   named `<engine>-review.md` / `<engine>-plan.md` (e.g.
   `.claude/plans/reviews/<wave>/claude-review.md`), never `claude.md`: Claude Code loads
   any `claude.md` as a nested `CLAUDE.md` on macOS's case-insensitive filesystem. If the
   Codex review pass hits a capacity or rate error, the gate retries once and then runs
   single-engine (see `.claude/commands/code-review.md`); record that in the wave-end note.
   **UI routing:** design-heavy UI (`site/`, landing pages, web (non-Rust) visual
   components) is built by the agent `bash .claude/scripts/routing.sh ui` names
   (`impl-coder-ui`, `implementation/coder-ui.md`: Opus, `frontend-design:frontend-design`
   skill), which verifies its own work in a browser against the named design reference;
   Codex still reviews it, and the Rust crate stays with the Implementer
   `routing.sh implementation` names (`implementation/orchestrator.md` → UI implementation
   routing). UI waves also get the
   catalogue survey at planning and the reference-fidelity check in the visual gate.
   **Live smoke:** a wave that builds a test harness driving a real TUI, agent CLI or network
   service runs it live once before the PR (`implementation/orchestrator.md` → Live smoke).
   **Stopping a Codex-backed agent** (e.g. to re-brief it) first runs the check in
   `implementation/orchestrator.md` → Role 1 → "Check before stopping an Implementer"
   (worktree status, stashes, Codex job; ask the agent and wait one reply cycle), then the
   cancel path in `implementation/coder-codex.md` → "Cancelling a run"; TaskStop alone leaves
   Codex editing the worktree.
4. **Pre-push check.**
   `bash .claude/scripts/quality-gate.sh && bash .claude/scripts/suite-lock.sh cargo test --locked`
   (or `/run-ci`). Fix red before pushing; a red CI cycle costs a two-runner round trip. The
   gate runs rustfmt and clippy `-D warnings` and, on macOS, clippy again for
   `x86_64-unknown-linux-gnu`, which host clippy skips (`cfg(target_os = "linux")`; W1's CI
   failed on an import unused there). Never run two test suites at once in one target dir: a
   concurrent `cargo build --example` re-links the example binaries a running suite uses.
5. **Open the PR into `main`.** `git push -u origin <branch>`, then `gh pr create --base
   main --title "W<n>: <summary>" --body <scope, tests, board issue (W<n> + document id)>`.
   Flip the issue → `in_review`. Record the PR number in the ledger.
6. **Poll.** Either `gh pr checks <N> --watch` (blocks until every check finishes) or a
   ScheduleWakeup ~270s apart (under the 5-minute prompt-cache window). Each cycle:
   - `gh pr checks <N>` — both `check (ubuntu-latest)` and `check (macos-latest)`.
   - New comments since the cursor:
     `gh api repos/nodera-studio/glen/pulls/<N>/comments`,
     `gh api repos/nodera-studio/glen/issues/<N>/comments`,
     `gh pr view <N> --json reviews,mergeable,mergeStateStatus`.
   - Update the poll cursor in the state file.
   On red CI: `gh run view <run-id> --log-failed` for the failing step.

   **Waiting on background work.** Whenever the session will idle longer than about 10 min on
   background agents, a workflow or CI, schedule a `ScheduleWakeup` heartbeat about every
   20 min. CI polling itself stays at about 270 s, as above.
   - On each wake, run the checkpoint check first ("Run checkpoints" above). Then compare
     against the previous wake, for every running agent:
     `git -C "$WT" status --short` and the last commit time (`git -C "$WT" log -1 --format=%cI`),
     the companion `status` where Codex runs, and the workflow and CI status.
   - Two wakes in a row with no change for an agent → run "Check before stopping an
     Implementer" (`implementation/orchestrator.md` → Role 1). Never use a wake as a reason to
     stop an agent by itself.
   - Once a day, and at every wave start, read the nightly flake proof:
     `gh run list --workflow flake-proof.yml --event schedule --limit 1 --json conclusion,headSha,url,createdAt`.
     A red run (its `headSha` on `main`) becomes a flake-ledger line in the Harness friction log
     and a tech-debt memory naming the failing tests from the run's summary. It never blocks a
     merge or a wave.
   - Stop scheduling heartbeats once everything has reported.

   **CI-owner marker.** When a background agent owns a PR's CI loop (it polls and fixes that
   PR), write `.claude/auto-mode/ci-owner/pr-<N>.json` in the main checkout:
   `{"owner":"<agent name>","since":"<date -u +%Y-%m-%dT%H:%M:%SZ>","head":"<sha>"}`. The
   directory is git-ignored (under `.claude/auto-mode/`).
   - On a CI-monitor event for PR N with a marker whose owner is still running, take no action
     yourself. Relay one line to the owner by SendMessage only for a failing check its last
     report did not name.
   - Delete the marker when the owner returns. A marker whose owner has finished is stale:
     delete it and handle the event yourself.
   - If the session's PR monitor can be paused for a delegated PR (`mcp__ccd_pr__set_monitor`;
     load its schema with ToolSearch first), pause it when you write the marker and resume it
     when you delete the marker.
7. **Triage + fix.** **Merge main at the start of every fix cycle**, before briefing any fix
   (CI, review or conformance), with `WT` = the wave's checkout:
   `git -C "$WT" fetch origin && git -C "$WT" merge --no-edit origin/main`. Never rebase.
   - Who merges: when a CI-owner agent (marker above) works in `$WT`, it merges main as its
     first fix step and you do not. Otherwise you merge, and only when
     `git -C "$WT" status --short` is empty; with uncommitted edits, have their agent commit
     or stash first.
   - A conflict goes to integration routing (`bash .claude/scripts/routing.sh integration`),
     recorded as in Shared seams item 4 below.
   - After a non-trivial merge, re-run `bash .claude/scripts/quality-gate.sh` and
     `cargo test --locked` before the fix. Name the merged `origin/main` SHA
     (`git -C "$WT" rev-parse origin/main`) in the fix brief.
   - Why: W9 was cut before W3b merged, so its live-route hint could not name `glen thinking`.
   - A merge changes the head SHA, so a SHA-bound proof (W10's flake proof) has to re-run on
     the new head.
   - **Squash-merged base.** A branch cut from, or merged with, another wave's pre-squash
     commit (Shared seams item 2) merges main with this recipe instead of the plain merge
     (verified by `.claude/scripts/tests/squash-merge-recipe-smoke.sh`):

     ```bash
     # The branch was cut from, or merged, wave A's pre-squash commit CUT; main has A's squash SQ.
     git -C "$WT" fetch origin "pull/<A>/head"                     # wave A's last tip, if not local
     A_TIP="$(gh pr view <A> --json headRefOid -q .headRefOid)"
     CUT="$(git -C "$WT" merge-base HEAD "$A_TIP")"
     SQ="$(gh pr view <A> --json mergeCommit -q .mergeCommit.oid)"
     # Exit 1 from a preview or the merge means conflicts; `|| …` keeps a `bash -e` shell going.
     git -C "$WT" merge-tree --write-tree --name-only HEAD origin/main || true                   # preview: plain base
     git -C "$WT" merge-tree --write-tree --name-only --merge-base="$CUT" HEAD origin/main || true  # preview: cut base
     git -C "$WT" replace --graft "$SQ" "$SQ^" "$CUT"   # temporary: SQ gets CUT as a second parent
     rc=0; git -C "$WT" merge --no-edit origin/main || rc=$?
     git -C "$WT" replace -d "$SQ"                       # always, right away, before resolving
     ```

     - The graft makes `git merge` pick the right base and leaves a normal index for any
       conflict left (`rc` 1), which goes to `routing.sh integration` as usual. The resulting
       merge commit has the ordinary parents (HEAD, `origin/main`).
     - `refs/replace` is shared by every worktree of the repo: run nothing else in the repo
       between the graft and its deletion, and never push `refs/replace`.
     - Why: W8, cut from W10's `042be28`, had 34 conflicts with the plain base and 13 with the
       cut base (memory 0c598d5a).

   Then: CI red → fix the cause (never relax a lint or a test to go green).
   Every fix-cycle decision, in the decision log and in fix briefs, quotes its governing AC:
   `Governing AC: AC-<id> — "<text copied verbatim from the plan file>"` (one line per AC), or
   `Governing AC: none — basis: <path:line> "<quoted line>"` (`implementation/orchestrator.md`
   → Fix cycles).
   A review comment with a real defect → fix, push, reply on the thread. A comment that
   is wrong → reply with evidence (file:line, a test) and resolve it. Theoretical-only →
   one-line reply with the reason. A Linux-only failure (paths, `dirs` locations) cannot
   be reproduced on macOS: read the log, fix from it, and let CI confirm.
8. **Capture, then merge the wave when the bar is met.** The bar: both `check` jobs
   green on the latest push, no unresolved review threads, `mergeable` true, and a final
   comment check right before merging. Then **store the wave's durable
   decisions/learnings** with `mcp__plugin_mnemosyne_memory__memory_store`
   (`project: "glen"`, `type: "semantic"` for a decision or `"episodic"` for what
   happened, `sourceKind: "decision"` or `"implementation"`, `PR #<N>` in the title,
   `decidedIn: "PR #<N>"` for decisions, file paths in `metadata.file`). Search first and
   update an existing memory rather than duplicating. The `capture-before-merge` hook
   blocks `gh pr merge` until such a call exists in the session. Then
   `gh pr merge <N> --squash` (prompt-gated; no `--delete-branch`).
9. **Close the wave.** Flip the issue → `done`; `document_tags_update` add
   `shipped:main`; `document_comment` the wave-end note (files changed, squash commit SHA,
   PR link, tests added). Write a wave-end Note (`document_create {projectId, kind:
   "notes"}`: what shipped, decisions, open follow-ups, what the next wave needs) and mark
   the wave's queue item `done`. Refresh the code index:
   `git -C ~/Developer/.mnemosyne-index/glen pull --ff-only`, then `code_reindex {project:
   "glen", repo: "glen"}`, confirmed with `code_index_status`. Update the ledger. Then
   `git push origin --delete <branch>` and `git switch main && git pull --ff-only`. If the
   wave ran in a worktree (a parallel wave, or a fix worktree), remove it per "Removing a
   worktree" below: cancel its Codex jobs, then `.claude/scripts/remove-worktree.sh`, which
   stops its brokers before removing it. Either way, finish with
   `bash .claude/scripts/sweep-codex-orphans.sh` and act on its task report: a finished job's
   `task` process can outlive it in a worktree that still exists. Last, stop every
   **Processes started** row the wave started (preview servers, watchers): confirm
   `ps -o lstart=,command= -p <pid>` still matches the row's start time and command (a reused
   pid is not yours), stop it, and fill the row's `stopped` with the time.
10. **Advance** to the next wave without asking. 2–3 line status at wave boundaries only;
    silent during polling.

A goal without a release is **done** once its last wave is merged, `done` and tagged
`shipped:main`, then "Closing the run". Only a goal that asks for a release continues to
step 11.

11. **Release (release goals only).** Set status `landing`. Run the `release` skill:
    version-bump PR (`Cargo.toml` + `Cargo.lock`, `CHANGELOG.md` if present) → poll CI
    the same way → capture a memory referencing the bump PR → `gh pr merge --squash` →
    `git tag -a v0.x.y -m "glen v0.x.y"` on that `main` commit → `git push origin v0.x.y`.
    Then `document_tags_update` add `released:v0.x.y` to every goal issue (and the goal
    module), then "Closing the run".

## Closing the run

Run this whenever a goal reaches `done`, with a release or without one, in this order. Status
becomes `done` only at its last item.

1. Keep checkpoints going ("Run checkpoints"); write the closing checkpoint into the closing PR.
2. Stop what the run started: every live **Processes started** row (pid, lstart and command
   still match), every run worktree through `remove-worktree.sh` ("Removing a worktree"), then
   `bash .claude/scripts/sweep-codex-orphans.sh`, acting on its task report.
3. Tech-debt memories: `memory_list` (`project: "glen"`, `sourceKind: "tech-debt"`, active). For
   each whose fix merged this run, cite the fix `path:line` on `origin/main` and `memory_update`
   it to `resolved`. Mark decisions this run reversed as `superseded`.
4. Board: the goal module → `done` (`document_update` with its current fields, as in "The
   Mnemosyne board"), and the run ledger queue's items `done`.
5. Deliverables meant for `main` (audit reports under `.claude/reviews/`) go into the closing PR
   after a gitleaks check, or the decision log says why not.
6. Scratch: `bash .claude/scripts/clean-scratch.sh --dry-run 'glen-*'`, then the real run for the
   entries this run created; list the rest for the owner.
7. Delete the run's merged PR branches. `git branch -r --merged origin/main` cannot find them
   (a squash merge leaves the branch unmerged) and lists `origin/main` itself, so take them from
   the run ledger's PR numbers: `gh pr view <N> --repo nodera-studio/glen --json
   state,headRefName -q 'select(.state == "MERGED") | .headRefName'` for each. Never delete
   `main` or `HEAD`; for each other name that `git ls-remote --exit-code --heads origin <branch>`
   still finds, `git push origin --delete <branch>`.
8. Last, after the closing PR merges: `CronList`, then `CronDelete` the id under
   **Checkpoints** → Schedule (load both with ToolSearch first), set `Cadence: none` and
   `Schedule: none`, set status `done`, and stop scheduling heartbeats.

Why: the v0.1.0 close left no checkpoints for 7.5 h, Codex brokers and an `astro preview`
running, fixed debt memories active, the module open, and the hourly prompt firing after the
run ended (`.claude/handoffs/v010-final-report.md`, checkpoint 41).

## Parallel work

### Harness self-improvement loop

The run fixes its own harness as it goes. Whenever something in `.claude/` slows or misleads
a wave, add a line to the state file's **Harness friction log** (and log a gap in
`.claude/orchestrator/gaps.jsonl` when no agent fits). With each wave PR, run one batch.
Plan the batch as independent fix groups by file (no two groups edit the same file) and run
it as a workflow: one implementer per group in its own worktree, one reviewer per group, and
the group branches merged into the batch branch (`.claude/docs/Workflow-Fanout.md`). Steps 1–4
describe the batch branch; each group worktree is cut and removed the same way.

Group briefs are sections of the batch plan file. A change during the run is written there, at
the top of the affected section, as `> REVISED <date -u +%Y-%m-%dT%H:%M:%SZ>: <what changed and
what to do>` or `> REVOKED <ISO>: <decision> — <replacement>`, before any message; every group
agent and the integrator record `shasum -a 256 <plan file>` when they start and re-hash it
before each commit and merge, applying every new REVISED or REVOKED line first (and stopping to
report when one contradicts work already committed). Group agents never read this file, so each
group brief carries that instruction itself, or names `coder-claude.md` → Step 3, item 3. A batch acceptance check that names an
open branch carries "once that branch is merged, run it against `origin/main`" (batch 10's W9
`merge-tree` gate went stale once W9 squash-merged).

1. `git worktree add .claude/worktrees/harness-<n> -b chore/harness-selfimprove-<n>
   origin/main` — an isolated worktree, so the wave checkout is untouched.
2. Fix the logged frictions and the open gaps, editing only harness files (`.claude/`) that
   the open wave branch does not touch (`git diff --name-only origin/main...<wave branch>`).
   A friction in a file the wave touches waits for the next batch.
3. External bugs (Mnemosyne, plugins, Codex runtime) are not fixed here: document the
   workaround in the skill or agent prompt that hits them.
4. Commit, push, PR into `main`, then poll, capture a memory and squash-merge exactly like a
   wave. Mark the fixed friction lines `fixed in PR #NN`, close the gaps, and record the batch
   in **Harness self-improvement**. Remove the worktree after the merge with
   `.claude/scripts/remove-worktree.sh`, then sweep (see "Removing a worktree" below).

### Independent waves in parallel

Waves can run in parallel; if their files overlap, follow Shared seams below. A parallel wave
is planned and built in its own worktree while the main checkout carries the current CLI wave:
`git worktree add .claude/worktrees/<slug> -b <branch> origin/main`, then run the per-wave
loop there. Each wave still PRs into `main` separately and is squash-merged on its own bar. If
the first merge conflicts with the later wave, bring `main` into it with
`git merge origin/main`; a wave that merged another wave's branch early never rebases.
W8-style fix groups (independent fixes split by file) run the same way as a harness batch: a
workflow with one implementer per group in its own worktree and one reviewer per group, merged
into the wave branch (`.claude/docs/Workflow-Fanout.md`).

**Shared seams.** W3 (live route) and W4 (agent surface) ran in parallel and both edited
`send.rs`, `doctor.rs`, `cli.rs` and the conformance command-surface rows, which cost a 16-file
semantic merge at PR time. So, when planning waves to run in parallel:

1. List each wave's touched files and seams from its plan (files to create or modify, plus
   shared surfaces: the clap command set in `cli.rs`, the visible-command set, doctor checks,
   the conformance command-surface rows) and flag every overlap in the state file's decision
   log. Build each list (one path per line) from the plan's "files to create/modify" section,
   or, once the wave's branch has commits, from `git diff --name-only origin/main...<branch>`:

   ```bash
   PLANS="$(cd "$(git rev-parse --path-format=absolute --git-common-dir)/.." && pwd -P)/.claude/plans"
   comm -12 <(sort -u "$PLANS/wave-a.files") <(sort -u "$PLANS/wave-b.files")
   ```

2. Overlapping waves either run in sequence, or the second wave merges the first wave's
   branch early: right after the first wave's review gate (`/implement` Role 2 green),
   `git -C <second worktree> merge <first branch>`, then re-run the gate and tests there. Not
   at PR time. Re-merge any later commits on the first wave's branch (its fix rounds) the
   same way. Once the first wave is squash-merged, the second brings `main` in with
   `git merge origin/main` (for a wave that merged another wave's branch early, with step 7's
   squash-merged base recipe), never a rebase, and only then opens or updates its PR.
3. A shared surface list (the visible-command set, doctor checks, the conformance
   command-surface rows) has one owner wave per run. Only the owner edits it; another wave
   hands its entries to the owner's plan or adds them after the owner merges.
4. Integrating two already-reviewed waves (the early merge, a conflict left at PR time, or a
   conflict from a fix cycle's main merge) goes to the agent
   `bash .claude/scripts/routing.sh integration` names (a Claude `general-purpose` agent when
   it prints `claude`), not necessarily the Implementer `routing.sh implementation` names,
   since it is merge resolution, not new product code (`implementation/orchestrator.md` →
   Role 1). When the two differ, record it as a routing deviation in the decision log and the
   PR body. Then re-run the gate and tests and a scoped Claude ‖ Codex `/code-review` of the
   resolved files.

`.claude/plans/` is git-ignored, so the new worktree does not contain the wave's plan. Every
brief for that wave (Implementer, fix, conformance, docs, reviewers) passes the plan by absolute
path in the main checkout
(`"$(git rev-parse --path-format=absolute --git-common-dir)/../.claude/plans/..."`, which
resolves to the main checkout from any worktree) and says its absence from the worktree is expected; otherwise Implementers report it "missing".
Design-heavy UI waves (the `site/`) follow the UI routing in step 3 above. After the wave's
squash-merge (step 9), remove its worktree, and any fix worktree cut for it, as below.

### Removing a worktree (stop its Codex processes)

Every Codex run in a worktree leaves a codex-companion broker (`app-server-broker.mjs serve
--cwd <worktree>`) and its app-server running after the run ends and after the worktree is
deleted; 7 such orphans were once killed by hand, and a finished job's `codex-companion.mjs
task` process was once still alive 8.5 h later. For each wave, fix or harness worktree you
remove, follow `implementation/orchestrator.md` → Role 1 → "Removing a worktree" with `WT` =
its absolute path, from the main checkout
(`MAIN="$(cd "$(git rev-parse --path-format=absolute --git-common-dir)/.." && pwd -P)"`):

1. Cancel `$WT`'s jobs with the companion first (`implementation/coder-codex.md` →
   "Cancelling a run", steps 0–1). The scripts never cancel jobs or signal tasks.
2. `bash "$MAIN/.claude/scripts/remove-worktree.sh" "$WT" [<branch>]` (`--dry-run` first if in
   doubt). It refuses unless `$WT` is a registered worktree directly under `.claude/worktrees/`
   and your cwd is outside it; with `<branch>`, unless that branch is checked out there and its
   tip is in `origin/main` or on its fetched upstream. It refuses while a live job (queued, or
   running with a live companion task pid, in either companion state store) or a task process
   is in `$WT`, printing the cancel commands; else it
   stops `$WT`'s brokers (TERM, wait, KILL), confirms nothing is left, copies every untracked
   or ignored file under `$WT`'s
   `.claude/{evidence,plans,research,handoffs,tech-debt,testing,reviews}/` to the same path in
   the main checkout (a different file already there is never overwritten: the worktree's copy
   goes under `.claude/evidence/from-worktrees/<worktree name>-<UTC stamp>/`; a failed copy
   removes nothing), then `git worktree remove --force` and `git branch -D`. Everything else
   uncommitted is discarded, so commit or push first. Never do these steps by hand or in
   another order.
3. `bash "$MAIN/.claude/scripts/sweep-codex-orphans.sh"`: stops this repo's brokers whose
   worktree is gone, with no live job or task process referencing it; lists suspected orphan
   tasks and stale job records without stopping them.

## Shipping tags (`shipped:main` / `released:v0.x.y`)

`done` on the board means "merged onto `main`". The tags make the release state explicit:

| Tag               | Meaning                                                   |
| ----------------- | --------------------------------------------------------- |
| `shipped:main`    | The issue's code is squash-merged on `main`               |
| `released:v0.x.y` | The issue's code is in the annotated tag `v0.x.y`         |

`released:*` accumulates with `shipped:main`; it does not replace it. Apply a tag only
when the merge or tag push actually happened this run — a tag tracks reality, not
intent. To find which issues a release contains when they weren't all part of this goal:
`git log <prev-tag>..v0.x.y --pretty=%s` and match the issues (`W<n>` label or document id) in the subjects.

## Autonomy before flagging — the escalation ladder

Deciding for yourself is the default; flagging is the exception. On anything unclear
or seemingly blocking, climb this ladder and stop at the first rung that resolves it:

1. **Read the codebase and the docs.** `AGENTS.md`, `docs/design.md`, `.claude/rules/`,
   the omp reference clone at `../metis/references/oh-my-pi` (read-only), and Mnemosyne
   (`memory_search` in `glen`; omp research lives in `metis`).
2. **Check the tools you actually have before declaring anything blocked.** Probe
   (`command -v`, a dry-run list call) before concluding a tool is missing:
   - `gh` — PRs, checks, run logs, comments, merges.
   - The Mnemosyne workspace tools — board, issues, comments, tags.
   - The Mnemosyne memory/codebase tools — decisions, prior art (`project: "glen"`).
   - `cargo` and `omp` (`omp --version`) — reproduce locally with `cargo run -- <args>`.
   - `npx ctx7` / `/find-docs` — current crate docs; the Exa MCP / `/research` — the web.
3. **Dispatch a specialist to decide for you.** Design/crate question → `/research` or
   the `plan-architect` / `plan-explorer` agent; a hard bug → `/deep-debug` or
   `codex:codex-rescue`. Record the decision + basis in the Decision log.
4. **Only then flag the user**, and only for true human-required blockers.

## Real blockers (flag fast, with a choice)

Flag immediately — stop grinding — when progress genuinely requires a human and no
rung above resolves it:

- A credential or account action only the user can do (an expired Mnemosyne token, `gh`
  auth, an omp login).
- A product/scope decision with material trade-offs research cannot settle and that is
  expensive to get wrong (e.g. a wave that would change the `--json` contract another
  tool depends on).
- External approval or access you cannot grant yourself (repo permissions, a GitHub
  setting).
- A wave PR red 3+ poll cycles with no path forward you can find.

Flag via `AskUserQuestion` with concrete options (recommended first, labeled
"(Recommended)"), state what you tried on the ladder, and where safe keep other
independent waves moving rather than idling the whole loop. Set status `blocked` and
record the open question in the state file so a context refresh does not lose it.

## Stop conditions

- Finish line met → "Closing the run" (status `done` at its last item), final summary (waves
  shipped, PRs, issues (`W<n>` + document id), anything deferred to tech debt). Goal without
  a release: last wave merged + `shipped:main`. Release goal: tag pushed + issues tagged
  `released:v0.x.y`.
- A wave PR red 3+ cycles with no path forward → stop that wave, flag the user.
- User says stop/pause or changes the goal → stop, confirm current state.

## Reporting cadence

Brief is kind — the user is working in parallel. 2–3 line status at each wave start
and end, and on real state changes (CI flipped, new comments, push, merge, blocker,
release tagged). Silent through quiet polling cycles. Always restate the goal and the
current wave in status lines so the user can re-orient.
