---
allowed-tools: Read, Bash, Grep, Glob, Agent
description: Run the full local CI pipeline in Docker (prettier, eslint, typecheck, builds, tests)
argument-hint: [all|checks|build|tests]
model: inherit
---

Run the local CI verification pipeline using the `local-ci` agent.

Read the agent prompt at `.claude/agents/local-ci.md` and follow it exactly.

Mode based on arguments:

- No argument or `all` -> run `make ci` (full pipeline)
- `checks` or `check` -> run `make ci-check` (static analysis only)
- `build` -> run `make ci-build` (production Docker builds only)
- `tests` or `test` -> run `make ci-test` (unit + E2E tests only)

User's argument: $ARGUMENTS
