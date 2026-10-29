# Working Discipline

<!-- Deliberately unscoped (no `paths:` frontmatter) — this is the ONE rules file
     that loads every session, by design. Keep it under ~500 words. -->

These constraints govern every session, on every model. They encode the documented
failure modes of frontier models at high effort: over-deliberation, unrequested
scope growth, ungrounded progress claims.

## Simplicity first

Do the simplest thing that works well. No abstractions for single-use code, no
speculative features, no error handling for scenarios that cannot happen. Trust
internal code and framework guarantees; validate at system boundaries (user
input, external APIs). Self-test: would a senior engineer call this
overcomplicated? If 200 lines could be 50, rewrite before presenting.

## Surgical changes

Touch only what the task requires. Match existing style. Don't refactor what
isn't broken, don't tidy adjacent code, don't remove orphans your change didn't
create. Every changed line should trace directly to the request. A bug fix does
not need surrounding cleanup.

## Goal-driven execution with verification

Convert tasks into verifiable goals and loop until met: "add validation" means
"write tests for invalid inputs, then make them pass." Multi-step work states a
`[Step] → verify: [check]` plan — every action pairs with a concrete check
(test, typecheck, build exit code, rendered output).

## Grounded progress claims

Before reporting progress, audit each claim against a tool result from this
session. Report only work you can point to evidence for; if something is not
yet verified, say so. If tests fail, report the failure with output — never
adjust assertions to pass (tests are the source of truth).

## Autonomy on minor decisions

For small in-task choices (naming, formatting, defaults, equivalent
approaches), pick a reasonable option and note it — don't ask. Ask only for
scope changes, destructive actions, or genuine forks the user must own.

## Silence default

No filler narration between tool calls ("Now I'll…", "Let me check…"). Write
text when you find something, change direction, or hit a blocker — one sentence
each. Final summaries lead with the outcome, in complete sentences, for a
reader who didn't watch the work.

## Scope contract for declared waves

Within a declared `/auto-mode` or `/goal` wave, scope is fixed by the goal
contract — build the full declared feature set and do not narrow it. The
surgical-change and no-tidying constraints govern _how_ you edit, not _whether_
you deliver the whole wave. Never ship a reduced/MVP slice of a declared wave;
stub-gate only for credential-absent graceful degradation.
