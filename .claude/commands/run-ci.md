---
allowed-tools: Read, Bash, Grep, Glob, Agent
description: Run glen's local CI pipeline with cargo (rustfmt check, clippy -D warnings, check, build, tests), mirroring ci.yml
argument-hint: [all|checks|build|tests]
model: inherit
---

Run the local CI verification pipeline using the `local-ci` agent. It mirrors
`.github/workflows/ci.yml` on the host with cargo; glen has no Docker and no Makefile.

Read the agent prompt at `.claude/agents/local-ci.md` and follow it exactly.

Mode based on arguments:

- No argument or `all` → full pipeline: `cargo fmt --all -- --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo check --all-targets --locked`, `cargo build --locked`, `cargo test --locked`
- `checks` or `check` → static checks only: fmt check, clippy, check
- `build` → `cargo build --locked`
- `tests` or `test` → `cargo test --locked`

User's argument: $ARGUMENTS
