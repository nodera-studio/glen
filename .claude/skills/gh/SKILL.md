---
name: gh
description: >-
  Use the `gh` CLI (2.94.0) for everything on the GitHub API surface — pull requests, issues,
  checks, and run logs. Reach for it whenever the user wants to open a PR, check PR status or CI
  checks, view or comment on a PR, list issues by label, view a workflow run's logs, or merge a
  PR. Trigger on indirect phrasing too — "send this up for review", "did CI pass", "what's
  failing on the PR", "show me the run logs", "what issues are tagged bug", "land it". `gh` IS
  the canonical PR/issue/checks tool — do NOT route plain git operations (commit, push, branch,
  rebase) through it; git stays git, gh is only for the GitHub-API surface on top of it. `gh pr
  merge` is a mutator and stays prompt-gated.
---

# gh — the GitHub API surface

`gh` is already the tool for PRs, issues, checks, and run logs. This skill exists to make that
routing explicit and to keep the git/gh boundary clean.

## The PR / issue / CI lifecycle

- **Pull requests:** `gh pr create`, `gh pr status`, `gh pr checks` (CI state), `gh pr view`,
  `gh pr diff`, `gh pr comment`.
- **Issues:** `gh issue list --label <label>`, `gh issue view`, `gh issue create`.
- **CI runs:** `gh run list`, `gh run view <id> --log` (failed-job logs), `gh run watch`.

## The git / gh boundary (do NOT cross it)

- **git stays git** — `commit`, `push`, `branch`, `rebase`, `merge` (local) are git commands, not
  `gh`. Do **not** steer git → gh. `gh` only wraps the GitHub _API_ (PRs, issues, checks, runs).
- Push is HTTPS-by-default via the credential helper (behavioral preamble). For commit message
  and trailer policy, defer to `.claude/CLAUDE.md` + `.claude/rules/behavioral-preamble.md` — do
  not restate it here (single source of truth, avoid drift).

## Guardrails

- **`gh pr merge` is a mutator → prompt-gated.** Never auto-merge; pushing + opening/updating the
  PR + watching CI is the default loop, the merge is a deliberate step.
- Never `--no-verify`, never force-push a shared branch (preamble invariant #5).
