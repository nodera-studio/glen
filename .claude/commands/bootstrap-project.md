---
allowed-tools: Read, Write, Edit, Bash, Glob, Grep, Agent, Skill
description: Bootstrap the agent-os harness onto a new or existing project — explore the stack, fill in template tokens, report a follow-up list
argument-hint: [path to target project, defaults to cwd]
model: inherit
---

Dispatch the bootstrap orchestrator to learn the target project and wire this harness onto
it, then relay its report.

Use the Agent tool to dispatch `.claude/agents/bootstrap/orchestrator.md` — pass it the
target path verbatim and relay its findings and follow-up list back to the user.

Target project path: $ARGUMENTS

If no arguments were provided, default to the current working directory and say so before
dispatching.

## What the orchestrator does

1. **Explore.** Run an Explore pass (and a Research pass where the stack involves
   unfamiliar libraries or conventions) over the target project to learn its stack: language(s),
   package manager, monorepo layout, test/build/lint commands, branch model, deployment
   target, and any existing `CLAUDE.md` or agent config to preserve rather than overwrite.
2. **Fill in tokens.** Substitute this harness's `{{TOKEN}}` placeholders across
   `CLAUDE.md.template`, `settings.json`, and any optional hooks with values learned in step 1.
   Representative tokens (the orchestrator's own token list is authoritative — this is
   illustrative, not exhaustive):
   - `{{PROJECT_NAME}}` — the project's name
   - `{{STACK_SUMMARY}}` — one-paragraph stack description
   - `{{PACKAGE_MANAGER}}` — npm/pnpm/yarn/cargo/pip/etc.
   - `{{DEV_COMMAND}}` / `{{TEST_COMMAND}}` / `{{LINT_COMMAND}}` / `{{BUILD_COMMAND}}`
   - `{{BRANCH_MODEL}}` — trunk name + release branch, if any
   - `{{PRIMARY_LANGUAGE}}` — main language/framework for tool-routing defaults
3. **Quality-check.** Rather than inventing its own bar for the written `CLAUDE.md`, the
   orchestrator defers to Anthropic's own `claude-md-management` plugin (install via
   `/plugin install claude-md-management@claude-plugins-official` if missing) — its
   `claude-md-improver` skill audits the file against quality criteria and proposes
   targeted additions, which get folded in (without reintroducing anything the bootstrap
   deliberately dropped). It also flags `/revise-claude-md` as the ongoing habit for
   capturing post-session learnings.
4. **Report.** Summarize what was filled in, what was left as a placeholder for the user to
   supply by hand (e.g. secrets, external service names), and a short follow-up checklist
   (missing hooks to wire, skills to enable/disable, anything ambiguous in the explore pass
   that needs a human call).

This command does not touch the target project's own source — it only writes/edits this
harness's config files (`CLAUDE.md`, `settings.json`, hooks) inside the target project's
`.claude/` directory.

## Observation

This command is stateless per invocation: the orchestrator's report is returned inline and
reviewed by the caller on every run. There is no telemetry to capture beyond the report — the
written `CLAUDE.md`/`settings.json` in the target project are themselves the observable
artifact; diff them after a run to confirm the substitution was correct.

## Feedback

To correct a poor bootstrap (wrong stack detected, tokens filled with guesses instead of
verified facts), refine the target path or re-run after fixing whatever confused the explore
pass (e.g. an out-of-date lockfile). The command does not learn between runs, so there is no
stored feedback loop to tune — each invocation re-explores from scratch.

## Rollback

`git rm .claude/commands/bootstrap-project.md` removes the command; the wrapped orchestrator
at `.claude/agents/bootstrap/orchestrator.md` is independent and unaffected. Changes it made
to the target project's `.claude/` files are ordinary edits — revert them with `git checkout --`
or `git revert` in the target project's own repo like any other change.
