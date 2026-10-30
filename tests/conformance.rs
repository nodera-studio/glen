//! Conformance suite: behaviour derived from the approved plans and `docs/design.md`, written
//! without reading the implementation.
//! - W0 (`.claude/plans/v010-wave-0-foundation.md`, AC-001…AC-029): cli_surface, color, doctor,
//!   fixtures, output_contract, paths, repo_contracts, sanitize, state, width, write_failure.
//! - W1 (`.claude/plans/v010-wave-1-reader.md`, AC-101…AC-126; §A3, §A8–§A11): ls, show, tail,
//!   lease, with the shared `reader` harness.
//!
//! Run: `cargo test --locked --test conformance`
//!
//! Every case drives the built `glen` binary with a cleared environment (env set per
//! `Command`, never process-wide), temp dirs for HOME/GLEN_HOME/cwd, and omp data only from
//! `tests/fixtures/`. TTY-only behaviour (pretty JSON, TTY spinner, terminal width) is not
//! covered here because the suite has no pseudo-terminal.

#[path = "conformance/reader.rs"]
mod reader;
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
#[path = "conformance/lease.rs"]
mod lease;
#[path = "conformance/ls.rs"]
mod ls;
#[path = "conformance/output_contract.rs"]
mod output_contract;
#[path = "conformance/paths.rs"]
mod paths;
#[path = "conformance/repo_contracts.rs"]
mod repo_contracts;
#[path = "conformance/sanitize.rs"]
mod sanitize;
#[path = "conformance/show.rs"]
mod show;
#[path = "conformance/state.rs"]
mod state;
#[path = "conformance/tail.rs"]
mod tail;
#[path = "conformance/width.rs"]
mod width;
#[path = "conformance/write_failure.rs"]
mod write_failure;
