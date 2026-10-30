---
name: release
description: >-
  glen's release runbook: a version-bump PR into `main`, then an annotated `v0.x.y` tag on the
  merged commit. Use this WHENEVER the user wants to cut a release, tag a version, "tag v0.x.y",
  "ship a version", "bump the version", "publish 0.2.0", or make a patch release after a hotfix,
  even when they don't say "tag". It enforces the details that are easy to get wrong: the bump
  goes through a PR like any other change (never a direct push to `main`), the tag is annotated
  and points at the squash-merge commit on `main`, a Mnemosyne memory referencing the PR is
  stored before merging, and the board issues get a `released:v0.x.y` tag afterwards.
---

# Release — tag a version on `main`

glen has no release branch. A release is an **annotated tag `v0.x.y` on a commit already on
`main`**, made after a version-bump PR merges. Nothing promotes between branches, so there is
nothing to drift; the two things that go wrong are tagging the wrong commit and skipping the PR.

## Versioning

- Semver, pre-1.0: `0.MINOR.PATCH`. A breaking CLI change (renamed subcommand, changed `--json`
  shape) bumps MINOR; fixes and additive changes bump PATCH. The tag is the version with a `v`
  prefix: `Cargo.toml` `version = "0.2.0"` ↔ tag `v0.2.0`.
- The version lives in `Cargo.toml` `[package] version`. `Cargo.lock` records it too, so both
  change in the bump.

## Runbook

1. **Preflight.**
   - `git fetch origin --tags && git switch main && git pull --ff-only`.
   - `gh run list --branch main --limit 1` — the latest `ci` run on `main` must be green.
   - Find the previous tag: `git describe --tags --abbrev=0 2>/dev/null` (none before the first
     release). List what ships: `git log <prev-tag>..origin/main --oneline` (or the whole history
     for the first release).
   - Confirm the tag does not exist yet: `git ls-remote --tags origin "refs/tags/v<version>"`
     prints nothing.
2. **Version-bump PR.**
   - `git switch -c chore/release-v<version>`.
   - Set `version = "<version>"` in `Cargo.toml`, then run `cargo build` (without `--locked`)
     so `Cargo.lock` records the new version. Commit both files.
   - If a `CHANGELOG.md` exists, add a `## v<version> — <YYYY-MM-DD>` section from the log in
     step 1. If none exists, put the summary in the PR body instead; don't create one unasked.
   - Pre-push: `cargo fmt --all -- --check && cargo clippy --all-targets --locked -- -D warnings
     && cargo test --locked`.
   - `gh pr create --title "chore: release v<version>"` with a body listing the shipped changes
     and the board issue keys they implement.
3. **Gate.** Wait for the `ci` checks (Linux and macOS) to go green: `gh pr checks <n> --watch`.
   Fix red on the same branch; never merge over red.
4. **Capture before merging.** `memory_store` with `project: "glen"`, `type: "episodic"`,
   `sourceKind: "implementation"`, a title like `Release v<version> (PR #<n>)`, and content
   listing the shipped changes and anything a user must do after upgrading. The
   `capture-before-merge` hook blocks `gh pr merge` until such a call exists.
5. **Merge.** `gh pr merge <n> --squash` (prompt-gated). Then `git switch main && git pull
   --ff-only`, and confirm `HEAD` is the squash commit: `git log -1 --oneline` shows the
   `chore: release v<version>` subject and `Cargo.toml` has the new version.
6. **Tag.** On that exact commit:
   `git tag -a v<version> -m "glen v<version>"` then `git push origin v<version>`. Pushing a tag
   is a mutator and stays prompt-gated. Never tag a commit that is not on `main`, never move or
   re-push an existing tag; if the tag is wrong, stop and ask the owner.
7. **Post-tag.**
   - If `.github/workflows/` has a workflow triggered by tags (none today; only `ci.yml`), watch
     it with `gh run list --limit 3` / `gh run watch <id>` and confirm it is green.
   - If the owner wants a GitHub Release: `gh release create v<version> --verify-tag --notes-file
     <file>` with the same notes as the PR body (only when asked).
   - Tag every board issue shipped in this release with `released:v<version>`
     (`document_tags_update`, `mode: "add"`, on the Mnemosyne board of project Glen; read the
     issue with `document_get` first for its `revision`). Keep their `shipped:main` tag.
   - Delete the release branch: `git push origin --delete chore/release-v<version>`.

## Hard rules

- The bump goes through a PR. Never commit or push to `main` directly, not even a version bump.
- Annotated tags only (`git tag -a`), on the squash-merge commit on `main`.
- Store the memory referencing the PR before `gh pr merge`.
- Never delete, move or force-push a tag that has been pushed.

## References

- `.claude/skills/branching-model/SKILL.md` — trunk-only branch, PR and merge rules.
- `.claude/skills/auto-mode/SKILL.md` — the wave loop runs this skill only when its goal asks for
  a release.
