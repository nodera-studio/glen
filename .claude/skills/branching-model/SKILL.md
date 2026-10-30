---
name: branching-model
description: >-
  glen's trunk-only branching rules. Consult this WHENEVER you cut a branch, open a PR, merge
  a PR, or pick a base branch, even in passing ("open a PR for this", "what do I branch off?",
  "merge it", "ship this fix"). In short: `main` is the only long-lived branch; branch off
  `main`, PR into `main`, squash-merge, never push to `main` directly; store a Mnemosyne memory
  referencing the PR before merging; releases are `v0.x.y` tags on `main` (the `release` skill).
  There is no staging branch and no release branch. Use it before git/PR/merge work so the base
  branch and merge method are right the first time.
---

# Branching model — `main` is the trunk

glen has one long-lived branch, `main`. There is no staging branch and no release branch, so
nothing promotes between branches and nothing can drift. A release is a tag on `main`.

## The rules

1. **Every branch cuts from `main`, every PR targets `main`.** `main` is the repo's default
   branch, so `gh pr create` targets it without `--base`.
2. **Squash-merge every PR** (`gh pr merge <n> --squash`). One PR is one commit on `main`.
3. **Never push to `main` directly**, including for hotfixes and version bumps. Every change,
   however small, goes through a PR so CI (`.github/workflows/ci.yml`: fmt, clippy and tests on
   Linux and macOS) runs on it.
4. **Releases are tags, not branches.** Tag `vX.Y.Z` on a commit already on `main`, after a
   version-bump PR. Use the `release` skill.

## Day-to-day

- **Start a branch:** `git fetch origin && git switch main && git pull --ff-only`, then
  `git switch -c feat/<slug>` (or `fix/<slug>`, `chore/<slug>`, `docs/<slug>`).
- **Before pushing:** the lefthook pre-push hook runs clippy and the tests; run
  `cargo fmt --all` first so the pre-commit check passes. Never `--no-verify` without a written
  reason.
- **Open a PR:** `gh pr create` into `main`. Keep each PR independently green; CI must pass on
  both runners.
- **Reference the board issue** the PR implements (a Mnemosyne board key, see `.claude/CLAUDE.md`
  → Issue tracking) in the PR title or body.
- **Before merging a green PR:** store the PR's durable decisions and learnings in Mnemosyne
  (`memory_store`, project `glen`), referencing `PR #<n>` in the title, content or tags. The
  `capture-before-merge` hook blocks `gh pr merge` until such a call exists in the session. A
  one-line episodic note is fine for a trivial PR.
- **Merge:** `gh pr merge <n> --squash`. `gh pr merge` is a mutator and stays prompt-gated.
  Delete the branch yourself afterwards (`git push origin --delete <branch>`), once the capture
  and the board update are done.
- **After merging:** move the board issue to `done` (see the `mnemosyne` skill).

## Hotfixes

A hotfix is an ordinary `fix/<slug>` branch off `main` and a PR into `main`. If the fix has to
ship to users, cut a patch release (`v0.x.(y+1)`) with the `release` skill once it is merged.

## Enforcement

- Branch protection on `main` (require a PR, require the `ci` checks, squash merges only) is a
  GitHub repo setting; confirm it is configured before relying on it.
- The `capture-before-merge` hook enforces the memory capture.

## References

- `.claude/skills/release/SKILL.md` — tagging a release.
- `.claude/skills/auto-mode/SKILL.md` — the autonomous multi-wave loop, also trunk-only.
