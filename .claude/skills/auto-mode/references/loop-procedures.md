# Auto-Mode — Detailed Loop Procedures

> Detailed sub-procedures extracted from `.claude/skills/auto-mode/SKILL.md`. The
> SKILL.md keeps the goal contract, the per-wave pipeline sequence, entry/exit
> conditions, and the hard rules. This file holds the long-form recipes:
> state-file template, wave-derivation detail, the per-wave step expansions,
> the shipping-state-label mechanics, the code-review-bot command hooks, the
> autonomy-before-flagging ladder, blocker criteria, and reporting cadence.
>
> Placeholders below (`{{STAGING_BRANCH}}`, `{{PROD_BRANCH}}`, `{{TICKET_PREFIX}}`) stand
> in for your project's actual branch names and issue-tracker ticket prefix — substitute
> your own throughout, or hardcode them once you adopt this skill in a real project.

## State file template

Create `.claude/auto-mode/state.md` from this on first write and keep it current at
every state change — it is your durable memory (the structured-note-taking pattern;
treat it like `progress.txt`):

```markdown
# Auto-Mode State

## Goal

<one-line objective — e.g. "Ship ticket {{TICKET_PREFIX}}-176 (cross-entity invoice labels) onto staging">

## Status

<goal-set | planning | in-progress | landing | blocked | done>

## Resolved goal

- Main ticket: <{{TICKET_PREFIX}}-XXX or "n/a">
- Goal kind: <feature (base {{STAGING_BRANCH}}) | promotion ({{STAGING_BRANCH}} → {{PROD_BRANCH}})>
- Wave source: <tracker-subissues | plan-doc | fresh-plan>
- Shipping label: <shipped:staging at each wave close (step 9); flipped to shipped:prod only on a {{STAGING_BRANCH}} → {{PROD_BRANCH}} promotion (step 11)>

## Wave ledger

| Wave | Ticket             | Branch          | PR → {{STAGING_BRANCH}} | State                                                  |
| ---- | ------------------ | --------------- | ------------------------ | ------------------------------------------------------ |
| 1    | {{TICKET_PREFIX}}-XXX | feat/...-wave-1 | #NN                     | merged \| in-progress \| polling \| blocked \| pending |

## Promotion PR (prod goals only)

- {{STAGING_BRANCH}} → {{PROD_BRANCH}}: <#NN or "n/a — staging-only goal"> — <state>

## Poll cursors

- PR #NN last-seen comment: <ISO timestamp>

## Blockers

- <none, or the open blocker + exactly what you asked the user>

## Decision log

- <date> <decision taken autonomously + basis (research/architect/codebase ref)>
```

## Resolving the goal into waves

**Tracker sub-issues first, plan fallback** (a reasonable default rule — adjust to your
own tracker's conventions):

1. Goal names a ticket (`{{TICKET_PREFIX}}-XXX`) → fetch via your issue-tracker MCP/CLI.
   Sub-issues/children exist → each child is a wave, in priority/manual order. Record
   `wave source: tracker-subissues`.
2. No children → look for an existing plan under `.claude/plans/` matching the
   ticket/feature with a wave breakdown. Use those waves. Record `plan-doc`.
3. Neither → run `/plan-implementation`, derive waves from the plan, then create one
   tracker sub-issue per wave under the main ticket (or the right project) so the
   user can track progress. Record `fresh-plan`.

Write the full wave ledger before starting Wave 1 so scope survives compaction.

## The per-wave loop — step expansions

Per wave, in order. The OUTER loop runs with no user gate between waves — that
autonomy is the point of this mode.

1. **Bootstrap.** `git fetch`, `git checkout {{STAGING_BRANCH}} && git pull`, branch
   `feat/<feature>-wave-{N}-{slug}` off `{{STAGING_BRANCH}}`. Flip the wave's tracker
   ticket(s) → `In Progress` per your tracker workflow conventions.
2. **Plan if needed.** `tracker-subissues` + non-trivial wave → run
   `/plan-implementation` scoped to this wave. Plan-doc/fresh-plan → plan exists,
   skip re-planning.
3. **Implement.** Invoke `/implement` for the wave. Let it own its internal gates.
4. **Pre-push check.** Run your local CI-check command (e.g. `make ci-check`); fix red
   before pushing (a pre-push hook should block otherwise, since a failed CI cycle
   typically costs several minutes of round-trip).
5. **Open the PR into `{{STAGING_BRANCH}}`** (`gh pr create --base {{STAGING_BRANCH}}`).
   Each wave must leave `{{STAGING_BRANCH}}` green and non-broken — gate any incomplete
   surface behind a flag.
6. **Poll continuously.** Wake ~270s apart (under a typical 5-min prompt-cache window).
   Each cycle: `gh pr checks` + fetch new inline review-comments, issue-comments,
   and reviews since the recorded last-seen timestamp; update the poll cursor.
   If you run an AI code-review bot (CodeRabbit or similar), it typically re-reviews
   every push; a bot like Cursor Bugbot posts a status check.
7. **Triage + fix.** Critical/Major actionable → fix, commit, push, reschedule.
   Minor → fix if cheap, else batch. Stale/contradictory → fix to preserve
   integrity. Theoretical-only → triage, usually skip with a one-line commit reason.
   Keep tests + docs in sync — a "missing test" comment from a review bot is a real
   finding.
8. **Capture, then merge the wave when the bar is met.** All checks green
   (Lint+Typecheck, Build, Unit+Integration, E2E, and any configured review bots), no new
   comments since the last push, and ≥1 clean cycle (zero new comments AND green CI) to
   defend the bot-posts-after-CI-greens race. THEN, if you run a durable-memory system,
   **before merging, store the wave's durable decisions/learnings referencing the PR**
   (`PR #NN` or the branch in the title/content/tags) — a pre-merge hook can block the
   merge until you do. Then `gh pr merge {N} --squash` into `{{STAGING_BRANCH}}` (NOT
   `--delete-branch` if you want deletion to stay deliberate — delete the branch at step
   9, after capture).
9. **Close the wave.** Flip tracker ticket(s) → `Done`, post the wave-end comment
   (files / commit / PR / tests) per your tracker workflow conventions. Update the
   ledger. Apply a **`shipped:staging`**-style label (the code is now on
   `{{STAGING_BRANCH}}`) per **Shipping-state labels** below. Then delete the wave branch
   (`git push origin --delete <branch>`).
10. **Advance** to the next wave without asking the user. 2–3 line status at wave
    boundaries only; silent during polling.

A staging-only goal is **done** once its last wave is merged + shipped-to-staging. Only a
goal that targets prod continues to step 11.

11. **Promote `{{STAGING_BRANCH}} → {{PROD_BRANCH}}` (prod goals only).** Set status
    `landing`. Open ONE PR `{{STAGING_BRANCH}} → {{PROD_BRANCH}}`; gate it on your
    CI-guard workflow (if configured — head must be `{{STAGING_BRANCH}}`) + the full CI
    suite — poll CI + bots the same way. When green and clean, merge with
    **`gh pr merge --merge`** (a merge commit — never squash/rebase a promotion, that
    drifts `{{PROD_BRANCH}}` from `{{STAGING_BRANCH}}`). Then **flip every goal ticket**
    (the main ticket + every wave sub-issue) `shipped:staging → shipped:prod` per
    **Shipping-state labels** below. Set status `done`. (Full runbook: the
    `promote-to-prod` skill.)

## Shipping-state labels (`shipped:staging` / `shipped:prod`)

`Done` in your tracker means "merged onto `{{STAGING_BRANCH}}`" (step 9) — it does NOT
say whether the code reached prod. Two mutually-exclusive labels make the real location
explicit and stop the same feature being rebuilt on the other branch (the "feature in
both main and staging" failure):

| Label             | Meaning                                       |
| ----------------- | ----------------------------------------------- |
| `shipped:staging` | Code is on `{{STAGING_BRANCH}}`, NOT yet on `{{PROD_BRANCH}}` |
| `shipped:prod`    | Code is live on `{{PROD_BRANCH}}` (production)  |

Set these up once in your issue tracker (as plain labels — no special ID scheme needed
beyond whatever your tracker assigns).

Flip, never accumulate: a ticket holds AT MOST one. `shipped:prod` supersedes
`shipped:staging`. `Done` + `shipped:staging` = merged to staging, awaiting prod.

**When the loop applies them.** `shipped:staging` at **wave close (step 9)**, AFTER the
wave's PR merges onto `{{STAGING_BRANCH}}`, for that wave's ticket. `shipped:prod` only
after the `{{STAGING_BRANCH}} → {{PROD_BRANCH}}` promotion merges (step 11), flipped for
EVERY ticket in the promotion (the main ticket + every wave sub-issue + parent epics):

- on a wave staging-merge → ensure the wave ticket has `shipped:staging`, no
  `shipped:prod`.
- on a `{{STAGING_BRANCH}} → {{PROD_BRANCH}}` promotion → flip each promoted ticket
  `shipped:staging` → `shipped:prod`.

If the merge does NOT happen this run (still polling, or blocked), apply NO shipping
label — the label tracks branch reality, not intent.

**Check whether your tracker's label-set write REPLACES the whole label set** (many do —
it's NOT additive like `blocks`/`links` typically are). If so, never pass just
`["shipped:staging"]`: that would strip `Feature`, `Bug`, etc. First read the ticket's
current labels, then pass the full desired set: keep the existing labels, drop the
opposite shipping label, add the target one. E.g. a ticket currently `["Feature"]`
landing on staging → save `["Feature", "shipped:staging"]`; later promoted to the release
branch → save `["Feature", "shipped:prod"]`.

**Promotion goals (`{{STAGING_BRANCH}} → {{PROD_BRANCH}}`).** A goal phrased "promote
staging → main" / "land the staging batch on main" is a step-11 promotion:
`{{STAGING_BRANCH}}` already carries many `Done`, `shipped:staging` tickets. On a
successful merge onto `{{PROD_BRANCH}}`, enumerate every ticket in the promotion diff
(`git log origin/{{PROD_BRANCH}}..origin/{{STAGING_BRANCH}} --pretty=%s | grep -oE
'{{TICKET_PREFIX}}-[0-9]+'`, plus their parent epics) and flip each `shipped:staging` →
`shipped:prod`. This is the moment the branch-divergence the labels exist to surface gets
resolved. (See the `promote-to-prod` skill.)

## Code-review-bot command hooks (drive the bot, don't wait on it)

If you run an AI code-review bot such as CodeRabbit, it typically auto-reviews every PR
into your release/staging branches and re-reviews on every push (check your bot's config
for `base_branches` + an auto-incremental-review setting), so you rarely need to force a
review. Drive it from the loop with `gh pr comment <N> --body "@coderabbitai <cmd>"` (or
your bot's equivalent mention syntax) at these points — never idle a poll cycle waiting
on it:

- **After you push fixes for ALL of the bot's actionable comments in a cycle** →
  `@coderabbitai resolve` (marks its threads resolved so the next poll sees a clean
  slate and the ≥1-clean-cycle merge bar is reached sooner).
- **Fallback — no bot status ~2 poll cycles after PR open, or on a PR it skipped** →
  `@coderabbitai review` (kick a review that didn't auto-trigger).
- **During a churn-heavy fixup burst** → `@coderabbitai pause` at the start,
  `@coderabbitai resume` once stable (stops re-review spam while you force-push several
  quick fixups). Always `resume` before evaluating the merge bar.
- Skip `full review`, `configure`, `generate docstrings` — manual/interactive only.

If your bot's config holds a PR in "changes requested" until its comments resolve, honor
it — the merge bar already requires zero new actionable comments, so fix or resolve,
never merge over an unresolved CRITICAL. If a pre-merge check blocks a wave on something
like docstring coverage, add the missing artifact (preferred) or, if it's genuinely noise
for that wave, note it in the decision log and relax the check in your bot's config
rather than wedging the loop.

## Autonomy before flagging — the escalation ladder

Deciding for yourself is the default; flagging is the exception. On anything unclear
or seemingly blocking, climb this ladder and stop at the first rung that resolves it:

1. **Read the codebase.** Most "unclear" is answerable from existing patterns,
   `.claude/rules/`, an architectural-decisions doc, or other reference codebases named
   in the project's CLAUDE.md.
2. **Check the tools you actually have before declaring anything blocked.** Probe
   (`command -v`, a dry-run list call) before concluding a tool is missing —
   "I assumed I couldn't" is not a reason to stop:
   - `gh` — PRs, checks, comments, merges, repo/issue API.
   - Your issue-tracker MCP/CLI — tickets, sub-issues, status, comments, projects.
   - Your database MCP/CLI — DB inspection, migrations, advisors, logs, types.
   - Your cloud/object-storage CLI (e.g. `wrangler` for Cloudflare) — object storage,
     workers, KV, D1 (a matching MCP may also be available for account/bucket reads).
   - Your hosting platform's CLI (e.g. `hcloud` for Hetzner) if present.
   - `npx ctx7` / `/research` / Context7 MCP — current library docs.
   - Any deployment-platform MCP (Vercel, etc.) — deployments, build/runtime logs.
     Billing/email/calendar/drive MCPs — only if a wave genuinely needs them.
3. **Dispatch a specialist to decide for you.** Design/library question → `/research`
   or the `plan-architect` / `plan-explorer` agent; let the finding settle it.
   Record the decision + basis in the Decision log. This is cheaper than a
   round-trip and is exactly what the user asked for.
4. **Only then flag the user**, and only for true human-required blockers.

## Real blockers (flag fast, with a choice)

Flag immediately — stop grinding — when progress genuinely requires a human and no
rung above resolves it:

- A credential/secret/external-account action only the user can do (set a prod env
  var, rotate a key, approve a billing-provider setting).
- A product/scope decision with material trade-offs research cannot settle and that
  is expensive to get wrong (e.g. a sub-issue conflicting with a hard product boundary —
  narrow or split?).
- External approval/access you cannot grant yourself (org permissions, a
  protected-branch override, a third-party allowlist).
- A wave PR red 3+ poll cycles with no path forward you can find.

Flag via `AskUserQuestion` with concrete options (recommended first, labeled
"(Recommended)"), state what you tried on the ladder, and where safe keep other
independent waves moving rather than idling the whole loop. Set status `blocked` and
record the open question in the state file so a context refresh does not lose it.

## Stop conditions

- Finish line met → status `done`, final summary (waves shipped, PRs, anything deferred
  to tech-debt). Staging-only goal: last wave merged + `shipped:staging`. Promotion goal:
  `{{STAGING_BRANCH}} → {{PROD_BRANCH}}` merged + tickets flipped `shipped:prod`.
- A wave PR red 3+ cycles with no path forward → stop that wave, flag the user.
- User says stop/pause or changes the goal → stop, confirm current state.

## Reporting cadence

Brief is kind — the user is working in parallel. 2–3 line status at each wave start
and end, and on real state changes (CI flipped, new bot comments, push, merge,
blocker, integration landed). Silent through quiet polling cycles. Always restate
the goal, base branch, and current wave in status lines so the user can re-orient.
