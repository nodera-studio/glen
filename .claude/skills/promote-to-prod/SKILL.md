---
name: promote-to-prod
description: >-
  The staging → release production-release runbook. Use this WHENEVER the user
  wants to ship to prod, promote staging to the release branch, cut a release, "push staging
  live", or land the staging batch on production — even when they don't name branches. It
  enforces the one-way-door details that are easy to get wrong: open the PR staging→release,
  wait for your CI-guard + CI-ok gates, capture a release memory BEFORE merging (if you run a
  durable-memory system), merge with a MERGE COMMIT (never squash/rebase — that drifts the release
  branch from staging), then flip every promoted ticket's shipping label and verify the prod
  deploy. Trigger on "ship to prod", "release to production", "promote to main", "deploy staging
  to prod", "cut a prod release".
---

# Promote to prod — staging → release branch

Production is reached **only** by promoting your staging trunk (e.g. `staging`) into your release
branch (e.g. `main`). The release branch never originates work, so this PR is always the staging
trunk exactly as it stands. The detail that bites people is the **merge method**: use a **merge
commit**, never squash or rebase. Squashing a long-lived branch writes a brand-new commit the
release branch doesn't share with staging, which instantly recreates "commits on the release
branch not on staging > 0" and drifts the two branches — the exact failure this model exists to
prevent. See your project's branching-model doc if one exists.

## Runbook

1. **Preflight — confirm staging is green and the release branch hasn't drifted.**
   - `git fetch origin {{PROD_BRANCH}} {{STAGING_BRANCH}}`
   - `git rev-list --count origin/{{PROD_BRANCH}}..origin/{{STAGING_BRANCH}}` — how many commits
     will promote.
   - `git rev-list --no-merges --count origin/{{STAGING_BRANCH}}..origin/{{PROD_BRANCH}}` — **must
     be 0**. `--no-merges` excludes prior promotion merge commits (which legitimately live on the
     release branch); any remaining count is a real feature commit that originated on the release
     branch, i.e. drift — stop and reconcile (back-merge to staging) before promoting.
   - Enumerate what ships: `git log origin/{{PROD_BRANCH}}..origin/{{STAGING_BRANCH}} --pretty=%s
     | grep -oE '{{TICKET_PREFIX}}-[0-9]+' | sort -u`.
2. **Open the promotion PR.** `gh pr create --base {{PROD_BRANCH}} --head {{STAGING_BRANCH}}` with
   a title like `Promote staging → main (<date / scope>)` and a body listing the promoted tickets
   plus the headline changes. There is no new branch — the head IS the staging trunk.
3. **Gate.** Wait for your CI-ok check AND your CI-guard workflow (if configured) to go green (`gh
   pr checks`). The guard confirms the head is the staging trunk. If CI is red, fix it on the
   staging trunk via a normal feature → staging PR, then re-evaluate — never bypass the gate or
   merge over red.
4. **Capture BEFORE merging (if you run a durable-memory system).** Store a release memory
   (reference the PR number) listing the promoted tickets and any operational follow-ups the
   release needs — migrations to run, env flips, feature-gate toggles. A pre-merge hook can block
   the merge until a memory referencing this PR exists. This is also where prod migration / env
   steps get recorded so they aren't forgotten post-deploy.
5. **Confirm with the user, then merge as a MERGE COMMIT.** This is an irreversible production
   release (the merge may auto-deploy to prod, flip labels, and/or require migrations). FIRST show
   the user the promotion diff — the ticket list from step 1 — and get an explicit go-ahead. Then
   `gh pr merge <N> --merge`. Never `--squash`, never `--rebase`. If your release-branch ruleset
   restricts the allowed method to merge-only, pick it explicitly anyway so intent is clear.
6. **Post-merge.**
   - Flip every promoted ticket and its parent epics from a "shipped:staging"-style label to a
     "shipped:prod"-style label in your issue tracker. Read each ticket's current labels first —
     a full-replace label API call needs the full desired set, not just the new label.
   - Verify prod auto-deploy fired (whatever your CD platform is) and the app is healthy.
   - Run any migrations the release memory flagged — migrations are typically **not**
     auto-applied.
   - Sanity: `git diff origin/{{PROD_BRANCH}} origin/{{STAGING_BRANCH}}` should be empty
     (tree-identical) right after the promotion.

## Hard rules

- **Merge commit only** — never squash/rebase a promotion, or the release branch and staging
  trunk drift.
- **Never commit directly to the release branch.** A hotfix branches off staging, PRs to staging,
  then promotes. If a direct-to-release-branch hotfix is truly unavoidable, label the PR with your
  CI guard's escape hatch (e.g. `emergency-hotfix`) and **back-merge release → staging
  immediately**.
- **Capture the release memory before merging** (if a hook enforces it).

## References

- Your project's branching-model doc, if one exists — the model, the two no-drift rules, the
  enforcement stack.
- `.claude/skills/auto-mode/SKILL.md` — the autonomous wave loop that feeds staging.
- `.claude/skills/branching-model/SKILL.md` — the day-to-day branch/PR/merge rules.
