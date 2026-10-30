//! Conformance suite: behaviour derived from the approved plans and `docs/design.md`, written
//! without reading the implementation.
//! - W0 (`.claude/plans/v010-wave-0-foundation.md`, AC-001…AC-029): cli_surface, color, doctor,
//!   fixtures, output_contract, paths, repo_contracts, sanitize, state, width, write_failure.
//! - W1 (`.claude/plans/v010-wave-1-reader.md`, AC-101…AC-126; §A3, §A8–§A11): ls, show, tail,
//!   lease, with the shared `reader` harness.
//! - W2 (`.claude/plans/v010-wave-2-rpc.md`, AC-201…AC-227; §A3–§A11, §A15): rpc, rpc_jobs, with
//!   the `rpc_support` harness driving the `fake_omp` example peer through `GLEN_OMP_BIN`.
//! - W3 (`.claude/plans/v010-wave-3-live.md`, AC-301…AC-327 incl. O1–O8, the O6 settlement
//!   amendment and the conformance clarifications; §A3–§A11, §A15): live,
//!   extension, inbox, w3_surface, with the `live_support` harness driving the `fake_ext` example
//!   peer and in-test unix-socket endpoints.
//! - W4 (`.claude/plans/v010-wave-4-agent.md`, AC-401…AC-413 and AC-419; O3, O5–O7; §A9–§A13 and
//!   `docs/json.md`): agent. The acceptance modes (AC-414…AC-418, AC-420, AC-421) need real omp.
//!
//! Run: `cargo test --locked --test conformance`
//!
//! Every case drives the built `glen` binary with a cleared environment (env set per
//! `Command`, never process-wide), temp dirs for HOME/GLEN_HOME/cwd, and omp data only from
//! `tests/fixtures/`. TTY-only behaviour (pretty JSON, TTY spinner, terminal width) is not
//! covered here because the suite has no pseudo-terminal.

#[path = "conformance/live_support.rs"]
mod live_support;
#[path = "conformance/reader.rs"]
mod reader;
#[path = "conformance/rpc_support.rs"]
mod rpc_support;
#[path = "conformance/support.rs"]
mod support;

#[path = "conformance/agent.rs"]
mod agent;
#[path = "conformance/cli_surface.rs"]
mod cli_surface;
#[path = "conformance/color.rs"]
mod color;
#[path = "conformance/controls.rs"]
mod controls;
#[path = "conformance/doctor.rs"]
mod doctor;
#[path = "conformance/ext_node.rs"]
mod ext_node;
#[path = "conformance/extension.rs"]
mod extension;
#[path = "conformance/fixtures.rs"]
mod fixtures;
#[path = "conformance/inbox.rs"]
mod inbox;
#[path = "conformance/lease.rs"]
mod lease;
#[path = "conformance/live.rs"]
mod live;
#[path = "conformance/ls.rs"]
mod ls;
#[path = "conformance/output_contract.rs"]
mod output_contract;
#[path = "conformance/paths.rs"]
mod paths;
#[path = "conformance/repo_contracts.rs"]
mod repo_contracts;
#[path = "conformance/rpc.rs"]
mod rpc;
#[path = "conformance/rpc_jobs.rs"]
mod rpc_jobs;
#[path = "conformance/sanitize.rs"]
mod sanitize;
#[path = "conformance/show.rs"]
mod show;
#[path = "conformance/state.rs"]
mod state;
#[path = "conformance/tail.rs"]
mod tail;
#[path = "conformance/w3_surface.rs"]
mod w3_surface;
#[path = "conformance/width.rs"]
mod width;
#[path = "conformance/write_failure.rs"]
mod write_failure;
