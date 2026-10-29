# Rules

Path-scoped conventions that Claude Code loads automatically — but only when they're
relevant, not on every turn. Two kinds live here, and the distinction matters:

## The one always-on rule

**`behavioral-preamble.md`** has no `paths:` frontmatter, which makes it the one rule
loaded in every session regardless of what files are touched. It's deliberately generic
(simplicity-first, surgical changes, grounded progress claims, silence-default) and
deliberately short (~500 words) — an unscoped rule is a per-turn tax, so this is the only
place that tax is worth paying. Don't add a second unscoped rule without a very good
reason; almost everything else belongs path-scoped below.

## Path-scoped rules (the pattern to copy)

Everything else should carry a `paths:` frontmatter array of globs — Claude Code
lazy-loads the file only when a matching path is read or edited in the current session.
This is how a large project keeps hundreds of lines of domain-specific convention
documented without paying to load all of it every turn.

```yaml
---
paths:
  - "apps/api/src/queues/**"
---
```

See [`example-scoped-rule.md.template`](example-scoped-rule.md.template) for the exact
shape to copy: rename it, replace the glob(s), replace the bracketed convention bullets,
delete the template note at the top.

## What a project rule is FOR

A rules file documents a convention that's true for one subsystem and would be noise
everywhere else — not a general engineering principle (that's `behavioral-preamble.md`'s
job), but this project's specific answer to a recurring question in one area. Examples
worth a rules file once your project has the corresponding subsystem:

- **A queue/job-processor convention** (`apps/**/queues/**`) — retry semantics, job-id
  naming, a DI footgun specific to your worker framework.
- **A database-schema convention** (`packages/db/**` or wherever migrations live) — how
  migrations pair with a schema file, a numbering-collision gotcha, a column-naming rule.
- **A shared-schema/validation convention** (`packages/shared/src/schemas/**`) — naming
  patterns for request/response schemas, a rule about where a schema must be exported from.
- **An E2E-testing convention** (`e2e/**` or `**/*.spec.ts`) — a flaky-selector gotcha
  specific to a UI library, a fixture-reset rule, a "never use networkidle" note.
- **An audit-log / compliance convention** — a fixed procedure for adding a new
  auditable action (e.g. "every new action needs a 2-file edit: the registry + the
  payload type, in the same change").

Each of these is real content someone will eventually want — none of it belongs in this
generic template, because it's specific to a stack this template doesn't assume you
have. Author your own once the corresponding subsystem exists in your project (or via
`/bootstrap-project`, which can stub these out once it's learned your stack).

## When NOT to add a rules file

If the convention applies everywhere (not to one path glob), it either belongs in
`behavioral-preamble.md` (if it's truly universal working discipline) or in `CLAUDE.md`
(if it's a project fact, not a behavioral constraint). A rules file that matches `**/*`
defeats the entire lazy-loading point — at that scope, just put it in `CLAUDE.md`.
