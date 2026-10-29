# Optional project-specific hooks

The source project this harness was extracted from had several hooks that
hardcoded its own dev-tool, container, and monorepo specifics. Those aren't
portable, so they were dropped rather than shipped as dead weight — but the
PATTERN each one demonstrated is worth knowing when you `/bootstrap-project`
a new repo and decide which project-specific hooks to author yourself.

| Hook name idea                    | Event                     | What it'd enforce                                                                                          |
| ---------------------------------- | -------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `enforce-<your-dev-tool>.sh`       | `PreToolUse` on `Bash`     | Blocks raw dev-server commands (e.g. bare `npm run dev`), steers to your project's canonical dev-start command (e.g. `make dev`). |
| `protect-<generated-artifact>.sh`  | `PreToolUse` on `Edit\|Write` | Blocks hand-edits to auto-generated files (migration snapshots, codegen output, lockfiles) that must only be produced by their generator. |
| `protect-<template-dir>.sh`        | `PreToolUse` on `Edit\|Write` | Blocks edits to scaffold/template source files from an unrelated task context (guards against accidental cross-contamination of generator templates). |
| `rebuild-<shared-package>.sh`      | `PostToolUse` on `Edit\|Write` | Rebuilds a shared internal package after an edit so dependent apps in a monorepo pick up the change immediately. |
| `stop-test-gate.sh`                | `Stop`                     | Blocks session end if typecheck/tests are failing when app code changed this session — forces a green state before the agent stops. |
| `worktree-<pkg-manager>-install.sh`| `PostToolUse` on `EnterWorktree` (and/or `SessionStart`) | Installs dependencies in a freshly created git worktree so it's immediately runnable. |

None of these are wired into `settings.json` by default. Author the ones your
project needs, following the same fail-open / non-blocking-unless-warranted
posture as the hooks that did ship (see `prettier-on-edit.sh`,
`shellcheck-on-edit.sh` for warn-don't-block examples, and
`capture-before-merge.sh` for a hard-block example).
