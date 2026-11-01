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
   - `gh run list --workflow ci --branch main --limit 1 --json conclusion,headSha,url` — the
     latest `ci` run on `main` must be green for the commit you will tag: `conclusion` is
     `success` and `headSha` equals `git rev-parse origin/main`. Keep `--workflow ci`: without
     it the newest run on `main` may be another workflow (the nightly flake proof, `release`).
   - Find the previous tag: `git describe --tags --abbrev=0 2>/dev/null` (none before the first
     release). List what ships: `git log <prev-tag>..origin/main --oneline` (or the whole history
     for the first release).
   - Confirm the tag does not exist yet: `git ls-remote --tags origin "refs/tags/v<version>"`
     prints nothing.
   - Dry-run the release workflow: `gh workflow run release.yml --ref main`, then `gh run watch`.
     It builds, smoke-tests, packages and test-installs all four targets without publishing; it
     must be green.
2. **Version-bump PR.**
   - `git switch -c chore/release-v<version>`.
   - Set `version = "<version>"` in `Cargo.toml`, then run `cargo build` (without `--locked`)
     so `Cargo.lock` records the new version. Commit both files.
   - If a `CHANGELOG.md` exists, add a `## v<version> — <YYYY-MM-DD>` section from the log in
     step 1. If none exists, put the summary in the PR body instead; don't create one unasked.
   - Pre-push: `cargo fmt --all -- --check && cargo clippy --all-targets --locked -- -D warnings
     && cargo test --locked`.
   - `gh pr create --title "chore: release v<version>"` with a body listing the shipped changes
     and the board issues they implement (`W<n>` + document id).
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
   - The tag push starts `release.yml`. It checks the tag (annotated, equal to `v` + the
     `Cargo.toml` version, on `main`), builds and verifies the four targets, and publishes the
     GitHub Release with generated notes. Watch it with `gh run list --workflow release.yml
     --limit 1` / `gh run watch <id>` and confirm it is green.
   - Confirm the 9 assets (4 `glen-<target>.tar.gz`, 4 `.sha256`, `SHA256SUMS`):
     `gh release view v<version> --json assets --jq '.assets[].name'`.
   - Install smoke: download this machine's asset, check its checksum and run it.

     ```bash
     case "$(uname -sm)" in
       "Darwin arm64") t=aarch64-apple-darwin ;;  "Darwin x86_64") t=x86_64-apple-darwin ;;
       "Linux x86_64") t=x86_64-unknown-linux-musl ;;  "Linux aarch64") t=aarch64-unknown-linux-musl ;;
     esac
     D="$(mktemp -d)"
     gh release download "v<version>" --repo nodera-studio/glen --dir "$D" \
       --pattern "glen-$t.tar.gz" --pattern "glen-$t.tar.gz.sha256"
     (cd "$D" && shasum -a 256 -c "glen-$t.tar.gz.sha256")
     tar -xzf "$D/glen-$t.tar.gz" -C "$D" && "$D/glen-$t/glen" --version   # prints: glen <version>
     ```

     `install.sh` itself is covered by the release workflow's `verify-install` job on both OSes
     (`release.yml:131`); the `steer-bash` hook blocks `curl`, so do not run `curl … | sh` here.
   - Fallback only if the workflow cannot run: `gh release create v<version> --verify-tag
     --notes-file <file>` with the same notes as the PR body, then upload the assets by hand.
   - After the first release (v0.1.0): set `releaseAvailable: true` in `site/src/data/site.ts` in a
     PR, so the site offers the install script and the latest release, and redeploy the site
     (`npm --prefix site run deploy`; its online link check verifies the script and the assets).
   - Tag every board issue shipped in this release with `released:v<version>`
     (`document_tags_update`, `mode: "add"`, on the Mnemosyne board of project Glen; read the
     issue with `document_get` first for its `revision`). Keep their `shipped:main` tag.
   - Delete the release branch: `git push origin --delete chore/release-v<version>`.
8. **Post-release checklist.**
   - The published release and its notes are checked: `gh release view v<version>` shows the
     generated notes and the 9 assets.
   - Every shipped board issue carries `released:v<version>` (Step 7).
   - Resolve the tech-debt memories whose fixes shipped in this release (`memory_update`, status
     `resolved`, with the fix `path:line`).
   - Stop any preview server or Codex broker the release work started: remove the release
     worktree with `bash .claude/scripts/remove-worktree.sh <absolute path>`, then run
     `bash .claude/scripts/sweep-codex-orphans.sh`.
   - Under `/auto-mode`, continue with `.claude/skills/auto-mode/references/loop-procedures.md` →
     "Closing the run" (checkpoints until the run ends, module close, schedule removal).

## Hard rules

- The bump goes through a PR. Never commit or push to `main` directly, not even a version bump.
- Annotated tags only (`git tag -a`), on the squash-merge commit on `main`.
- Store the memory referencing the PR before `gh pr merge`.
- Never delete, move or force-push a tag that has been pushed.
- The release is not done until the Step 7 install smoke prints the new version.

## References

- `.claude/skills/branching-model/SKILL.md` — trunk-only branch, PR and merge rules.
- `.claude/skills/auto-mode/SKILL.md` — the wave loop runs this skill only when its goal asks for
  a release.
