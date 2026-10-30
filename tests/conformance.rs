//! W0 conformance suite: behaviour derived from `.claude/plans/v010-wave-0-foundation.md`
//! (AC-001…AC-029) and `docs/design.md` §A1–§A17, written without reading the implementation.
//!
//! Run: `cargo test --locked --test conformance`
//!
//! Every case drives the built `glen` binary with a cleared environment (env set per
//! `Command`, never process-wide), temp dirs for HOME/GLEN_HOME/cwd, and omp data only from
//! `tests/fixtures/`. TTY-only behaviour (pretty JSON, TTY spinner, terminal width) is not
//! covered here because the suite has no pseudo-terminal.

#[path = "conformance/support.rs"]
mod support;

#[path = "conformance/cli_surface.rs"]
mod cli_surface;
#[path = "conformance/color.rs"]
mod color;
#[path = "conformance/doctor.rs"]
mod doctor;
#[path = "conformance/fixtures.rs"]
mod fixtures;
#[path = "conformance/output_contract.rs"]
mod output_contract;
#[path = "conformance/paths.rs"]
mod paths;
#[path = "conformance/repo_contracts.rs"]
mod repo_contracts;
#[path = "conformance/sanitize.rs"]
mod sanitize;
#[path = "conformance/state.rs"]
mod state;
#[path = "conformance/width.rs"]
mod width;
#[path = "conformance/write_failure.rs"]
mod write_failure;
