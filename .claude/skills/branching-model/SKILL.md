---
name: branching-model
description: >-
  The staging-trunk branching rules — consult this WHENEVER you cut a branch, open a
  PR, merge a PR, or pick a base branch, even in passing ("open a PR for this", "what do
  I branch off?", "merge it", "ship this fix"). The rules exist to prevent two specific
  failures: work originating on the release branch, and squash-promotions that drift the
  release branch from the staging trunk. In short: branch off your staging trunk, PR into
  it (squash-merge), never start work on the release branch; capture a durable memory
  before merging any green PR (if you have a memory system); reach prod only by promoting
  staging → release with a merge commit. Use it before doing git/PR/merge work so the base
  branch and merge method are right the first time.
---

# Branching model — staging is the trunk, the release branch is release-only

Two rules keep your release branch (e.g. `main`) and your staging trunk (e.g. `staging`) from
drifting. If your project has a fuller written doc for this model, link it here as the
authoritative source; this skill is the operating summary. Substitute your actual branch names
for `{{STAGING_BRANCH}}` / `{{PROD_BRANCH}}` below.

## The two rules

1. **Nothing originates on `{{PROD_BRANCH}}`.** Every branch cuts from `{{STAGING_BRANCH}}`;
   every feature PR targets `{{STAGING_BRANCH}}`. `{{PROD_BRANCH}}` only ever receives a
   `{{STAGING_BRANCH}} → {{PROD_BRANCH}}` promotion. So `{{PROD_BRANCH}}` is always a subset of
   `{{STAGING_BRANCH}}` and divergent work is impossible.
2. **Feature → `{{STAGING_BRANCH}}` is a squash-merge; `{{STAGING_BRANCH}} → {{PROD_BRANCH}}` is
   a merge commit.** Squash collapses a feature into one clean commit on the staging trunk. But
   squashing the long-lived staging branch into the release branch writes a new commit the
   release branch doesn't share with staging — which drifts them and makes the next promotion
   conflict. Promotions are **merge commits**, never squash/rebase.

## Day-to-day

- **Start a branch:** `git fetch && git checkout {{STAGING_BRANCH}} && git pull`, then
  `git checkout -b feat/<slug>` (or `fix/<slug>`). Default base is `{{STAGING_BRANCH}}` — if it's
  your repo's default branch, `gh pr create` targets it automatically.
- **Open a PR:** into `{{STAGING_BRANCH}}`. Keep each PR independently green — the staging trunk
  typically auto-deploys to a staging env on merge, so don't land a half-broken state (gate
  incomplete surfaces behind a flag).
- **Before merging a green PR:** if you run a durable-memory system, store the decisions/learnings,
  referencing the PR (`PR #<n>` or the branch in title/content/tags). A pre-merge hook can enforce
  this — the code is preserved by the merge, but the *knowledge* is lost unless you write it. A
  one-line note is fine for a trivial PR.
- **Merge:** `gh pr merge <n> --squash`. Don't auto-delete the branch in the same step if you want
  deletion to stay a deliberate, post-capture step; delete it yourself once the PR is closed out.
- **Ship to prod:** never push to `{{PROD_BRANCH}}` directly. Use the `promote-to-prod` skill —
  open ONE `{{STAGING_BRANCH}} → {{PROD_BRANCH}}` PR and merge it with `gh pr merge --merge`
  (merge commit).

## Hotfixes

A prod-critical fix still goes **through `{{STAGING_BRANCH}}` first**: branch off it, PR to it,
then promote. If a direct-to-`{{PROD_BRANCH}}` hotfix is genuinely unavoidable, label the PR to
match whatever escape-hatch label your CI guard workflow checks for (e.g. `emergency-hotfix`) and
**back-merge `{{PROD_BRANCH}} → {{STAGING_BRANCH}}` immediately**, or you have drifted by
definition. If your repo needs hard enforcement of "nothing originates on the release branch",
wire up a CI guard workflow that checks the PR head branch and blocks merges that don't come from
the staging trunk (minus the labeled escape hatch).

## References

- Your project's branching-model doc, if one exists — full model + enforcement (rulesets, default
  branch, guard workflow, capture hygiene).
- `.claude/skills/promote-to-prod/SKILL.md` — the staging → release runbook.
- `.claude/skills/auto-mode/SKILL.md` — the autonomous multi-wave loop (also staging-trunk).
