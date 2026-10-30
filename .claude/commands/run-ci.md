---
allowed-tools: Read, Bash, Grep, Glob, Agent
description: Run glen's local CI pipeline with cargo (rustfmt check, clippy -D warnings, check, build, tests), plus site checks, mirroring ci.yml
argument-hint: [all|checks|build|tests|site]
model: inherit
---

Run the local CI verification pipeline using the `local-ci` agent. It mirrors
`.github/workflows/ci.yml` on the host with cargo and the site npm scripts; glen has no Docker and no Makefile.

Read the agent prompt at `.claude/agents/local-ci.md` and follow it exactly.

Mode based on arguments:

- No argument or `all` → full pipeline: `cargo fmt --all -- --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo check --all-targets --locked`, `cargo build --locked`, `cargo test --locked`, and `shellcheck -s sh install.sh scripts/package-release.sh` (CI runs it on Linux; skip with a note when shellcheck is missing). `cargo test` includes the site demo golden (`tests/site_demo.rs`) and the README check (`tests/readme.rs`).
- `site` → Site stage: when `site/` exists, run `npm ci`, `npm run verify` (now ending in the offline `npm run links`) and `npm run test:e2e` from it; LHCI only on request. The online link check and `wrangler deploy --dry-run` belong to the CI `site-deploy` job and `npm run deploy`.
- `all` also includes Site when `site/node_modules` exists; otherwise report the missing-dependencies gap.
- `checks` or `check` → static checks only: fmt check, clippy, check
- `build` → `cargo build --locked`
- `tests` or `test` → `cargo test --locked`

User's argument: $ARGUMENTS
