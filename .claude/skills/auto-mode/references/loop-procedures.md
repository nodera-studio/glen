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

## Blockers

- <none, or the open blocker + exactly what you asked the user>

## Decision log

- <date> <decision taken autonomously + basis (research/architect/codebase ref)>
```

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
3. **Implement.** Invoke `/implement` for the wave. It owns its gates: the Codex
   Implementer's cargo loop and `.claude/scripts/quality-gate.sh`, the local
   `/code-review` gate (1 Claude ‖ 1 Codex pass, 2-vote), the conformance tail
   (`cargo test --test conformance`) and the docs updater. This local gate is the code
   review; nothing on GitHub reviews the PR. Saved review or plan files per engine are
   named `<engine>-review.md` / `<engine>-plan.md` (e.g.
   `.claude/plans/reviews/<wave>/claude-review.md`), never `claude.md`: Claude Code loads
   any `claude.md` as a nested `CLAUDE.md` on macOS's case-insensitive filesystem. If the
   Codex review pass hits a capacity or rate error, the gate retries once and then runs
   single-engine (see `.claude/commands/code-review.md`); record that in the wave-end note.
   **UI routing:** design-heavy UI (`site/`, landing pages, web (non-Rust) visual
   components) is built by the Claude UI Implementer (`impl-coder-ui`,
   `implementation/coder-ui.md`: Opus, `frontend-design:frontend-design` skill), which
   verifies its own work in a browser against the named design reference; Codex
   still reviews it, and the Rust crate stays with the Codex Implementer
   (`implementation/orchestrator.md` → UI implementation routing). UI waves also get the
   catalogue survey at planning and the reference-fidelity check in the visual gate.
   **Stopping a Codex-backed agent** (e.g. to re-brief it) first runs the check in
   `implementation/orchestrator.md` → Role 1 → "Check before stopping an Implementer"
   (worktree status, stashes, Codex job; ask the agent and wait one reply cycle), then the
   cancel path in `implementation/coder-codex.md` → "Cancelling a run"; TaskStop alone leaves
   Codex editing the worktree.
4. **Pre-push check.**
   `bash .claude/scripts/quality-gate.sh && cargo test --locked` (or `/run-ci`). Fix red
   before pushing; a red CI cycle costs a two-runner round trip. The gate runs rustfmt and
   clippy `-D warnings` and, on macOS, clippy again for `x86_64-unknown-linux-gnu`, which
   host clippy skips (`cfg(target_os = "linux")`; W1's CI failed on an import unused there).
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
7. **Triage + fix.** CI red → fix the cause (never relax a lint or a test to go green).
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
   worktree" below, which stops its Codex broker first.
10. **Advance** to the next wave without asking. 2–3 line status at wave boundaries only;
    silent during polling.

A goal without a release is **done** once its last wave is merged, `done` and tagged
`shipped:main`. Only a goal that asks for a release continues to step 11.

11. **Release (release goals only).** Set status `landing`. Run the `release` skill:
    version-bump PR (`Cargo.toml` + `Cargo.lock`, `CHANGELOG.md` if present) → poll CI
    the same way → capture a memory referencing the bump PR → `gh pr merge --squash` →
    `git tag -a v0.x.y -m "glen v0.x.y"` on that `main` commit → `git push origin v0.x.y`.
    Then `document_tags_update` add `released:v0.x.y` to every goal issue (and the goal
    module). Set status `done`.

## Parallel work

### Harness self-improvement loop

The run fixes its own harness as it goes. Whenever something in `.claude/` slows or misleads
a wave, add a line to the state file's **Harness friction log** (and log a gap in
`.claude/orchestrator/gaps.jsonl` when no agent fits). With each wave PR, run one batch:

1. `git worktree add .claude/worktrees/harness-<n> -b chore/harness-selfimprove-<n>
   origin/main` — an isolated worktree, so the wave checkout is untouched.
2. Fix the logged frictions and the open gaps, editing only harness files (`.claude/`) that
   the open wave branch does not touch (`git diff --name-only origin/main...<wave branch>`).
   A friction in a file the wave touches waits for the next batch.
3. External bugs (Mnemosyne, plugins, Codex runtime) are not fixed here: document the
   workaround in the skill or agent prompt that hits them.
4. Commit, push, PR into `main`, then poll, capture a memory and squash-merge exactly like a
   wave. Mark the fixed friction lines `fixed in PR #NN`, close the gaps, and record the batch
   in **Harness self-improvement**. Remove the worktree after the merge, stopping its Codex
   broker first (see "Removing a worktree" below).

### Independent waves in parallel

An independent wave (one sharing no files with the current wave, e.g. the `site/`) can be
planned and built in its own worktree while the main checkout carries the current CLI wave:
`git worktree add .claude/worktrees/<slug> -b <branch> origin/main`, then run the per-wave
loop there. Each wave still PRs into `main` separately and is squash-merged on its own bar;
rebase the later one if the first merge conflicts with it.

**Shared seams.** W3 (live route) and W4 (agent surface) ran in parallel and both edited
`send.rs`, `doctor.rs`, `cli.rs` and the conformance command-surface rows, which cost a 16-file
semantic merge at PR time. So, when planning waves to run in parallel:

1. List each wave's touched files and seams from its plan (files to create or modify, plus
   shared surfaces: the clap command set in `cli.rs`, the visible-command set, doctor checks,
   the conformance command-surface rows) and flag every overlap in the state file's decision
   log. With one path per line in each list:

   ```bash
   comm -12 <(sort -u "$PLANS/wave-a.files") <(sort -u "$PLANS/wave-b.files")
   ```

2. Overlapping waves either run in sequence, or the second wave merges the first wave's
   branch early: right after the first wave's review gate (`/implement` Role 2 green),
   `git -C <second worktree> merge <first branch>`, then re-run the gate and tests there. Not
   at PR time.
3. A shared surface list (the visible-command set, doctor checks, the conformance
   command-surface rows) has one owner wave per run. Only the owner edits it; another wave
   hands its entries to the owner's plan or adds them after the owner merges.
4. Integrating two already-reviewed waves (the early merge, or a conflict left at PR time) may
   go to a Claude agent (`general-purpose`) instead of the Codex Implementer, since it is
   merge resolution, not new product code. Record it as a routing deviation in the decision
   log and the PR body, then re-run the gate and tests and a scoped `/code-review` of the
   resolved files.

`.claude/plans/` is git-ignored, so the new worktree does not contain the wave's plan. Every
brief for that wave (Implementer, fix, conformance, docs, reviewers) passes the plan by absolute
path in the main checkout
(`"$(git rev-parse --path-format=absolute --git-common-dir)/../.claude/plans/..."`, which
resolves to the main checkout from any worktree) and says its absence from the worktree is expected; otherwise Implementers report it "missing".
Design-heavy UI waves (the `site/`) follow the UI routing in step 3 above. After the wave's
squash-merge (step 9), remove its worktree, and any fix worktree cut for it, as below.

### Removing a worktree (stop its Codex broker)

Every Codex run in a worktree leaves a codex-companion broker (`app-server-broker.mjs serve
--cwd <worktree>`) and its app-server running after the run ends and after the worktree is
deleted; 7 such orphans were once killed by hand. For each wave, fix or harness worktree you
remove, follow `implementation/orchestrator.md` → Role 1 → "Removing a worktree" with `WT` =
its absolute path:

1. The guarded cancel recipe (`implementation/coder-codex.md` → "Cancelling a run", steps
   0–3) with step 2 unconditional. It refuses an empty `WT` or the main checkout, matches the
   node broker only, and skips `$$`/`$PPID`.
2. `git worktree remove "$WT"` only after step 3's verify is clean. If the worktree is already
   gone, step 0 refuses; go straight to the sweep.
3. The self-contained orphan sweep from that section: it stops brokers whose `--cwd` no longer
   exists, only under this repo's `.claude/worktrees/`.

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

- Finish line met → status `done`, final summary (waves shipped, PRs, issues (`W<n>` +
  document id), anything deferred to tech debt). Goal without
  a release: last wave merged + `shipped:main`. Release goal: tag pushed + issues tagged
  `released:v0.x.y`.
- A wave PR red 3+ cycles with no path forward → stop that wave, flag the user.
- User says stop/pause or changes the goal → stop, confirm current state.

## Reporting cadence

Brief is kind — the user is working in parallel. 2–3 line status at each wave start
and end, and on real state changes (CI flipped, new comments, push, merge, blocker,
release tagged). Silent through quiet polling cycles. Always restate the goal and the
current wave in status lines so the user can re-orient.
