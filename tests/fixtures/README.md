# omp fixtures

Synthetic JSONL v3 sessions for omp 18.14.0. Never copy real session content; placeholder values only.
Tests point PI_CODING_AGENT_DIR at `omp/agent`, with a temp HOME. No test reads live ~/.omp.
Buckets represent `$HOME/work/alpha` and `$HOME/work/beta`; headers use `/home/fixture/work/{alpha,beta}`.

| Key | Path under sessions/ | Id | Handle | Exercises |
|---|---|---|---|---|
| A | `-work-alpha/2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.jsonl` | `01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de` | `e27c0de` | basic: thinking, toolCall/toolResult, model/thinking/title changes and usage |
| B | `-work-alpha/2026-10-02T14-30-00-000Z_01a0fd05-a240-7c41-9d02-7d2e8f13a5b6.jsonl` | `01a0fd05-a240-7c41-9d02-7d2e8f13a5b6` | `f13a5b6` | continuation parent; second user content is a string |
| C | `-work-alpha/2026-10-02T15-10-00-000Z_01a0fd2a-4140-7e05-a3f4-0c9a4b7e2d81.jsonl` | `01a0fd2a-4140-7e05-a3f4-0c9a4b7e2d81` | `b7e2d81` | continuation child; B entries copied verbatim |
| D | `-work-beta/2026-10-03T08-00-00-000Z_01a100c6-f000-7b18-8e6a-5e8b1c4f9a72.jsonl` | `01a100c6-f000-7b18-8e6a-5e8b1c4f9a72` | `c4f9a72` | branched; last entry activeLeafId selects label 40000008 |
| E | `-work-beta/2026-10-04T11-45-00-000Z_01a106bb-4a60-7f92-b0c3-b2d6e0a3f417.jsonl` | `01a106bb-4a60-7f92-b0c3-b2d6e0a3f417` | `0a3f417` | compaction, unknown fields/types/blocks, bashExecution and malformed lines |
| F | `-work-beta/2026-09-15T07-20-00-000Z_01a0a3ef-d900-7d6e-9a57-9b3d5a81c0de.jsonl` | `01a0a3ef-d900-7d6e-9a57-9b3d5a81c0de` | `a81c0de` | legacy header title; no slot |
| G | `-work-alpha/2026-10-05T16-20-00-000Z_01a10cdd-6b80-7a29-9c5e-6d1f3b8e4a07.jsonl` | `01a10cdd-6b80-7a29-9c5e-6d1f3b8e4a07` | `b8e4a07` | fork of A; parentSession only, never a continuation |
| S | `-work-alpha/2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de/subagent-01.jsonl` | `01a0f6b5-ba60-7012-8f3b-3f7a9c1e5b20` | n/a | nested subagent artifact, header only plus user message; discovery skips it |

Every non-legacy top-level session has a title slot of exactly 256 bytes including newline.
S is a nested artifact transcript, with no title slot. Only entries carry runtime.version; title slots and session headers do not.
B's string-form user message also appears verbatim in C's copied history. All other user content uses text-block arrays.
A and F share the last four hex digits `c0de` but have unique seven-hex tails. A and S share prefix `01a0f6b`.
D's active branch is 40000001, 40000002, 40000005, 40000006, 40000007, 40000008.
E has exactly two deliberately malformed lines: literal `not json at all` and the final truncated
`{"type":"message","id":"deadbeef","parentId":` (no trailing newline). Do not normalise its final newline.
The empty dotfile `.2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.jsonl.lock.os` is an append-gate sibling.
`bin/fake-omp` (0755) answers --version, supports FAKE_OMP_VERSION, and rejects other arguments with exit 64.

Run `cargo test --locked --test fixtures_sanity` to check the slots, headers, copied entries,
active branch, malformed lines, message shapes, handles and artifact layout.

Fixture mtimes are checkout times, not conversation timestamps. Tests that depend on ordering
copy fixtures with `tests/common::OmpBox` and set deterministic mtimes; never rewrite the checked-in fixtures.
Fixture A has nine raw entries after its title slot and session header.

## Fake RPC peer (`fake_omp` example)

`tests/support/fake_omp.rs` is an independent JSON-lines peer built by `cargo build --example fake_omp`.
`cargo test` builds examples; filtered integration runs build it on demand through `tests/common::fake_omp_bin`.
Tests point `GLEN_OMP_BIN` at its absolute path. It never writes a transcript or reads live `~/.omp`.

Modes: `--version`, `--mode rpc [--no-ui] [--profile NAME] [--resume FILE]`, and
`hold-lease PATH` (prints `held`, retains an omp owner lease until stdin EOF). Unknown arguments exit 64.

`FAKE_OMP_SCENARIO` selects `ok` (default), `local`, `progress`, `bad_reply`, `unsettled`,
`never_settles`, `error`, `aborted`, `crash`, `hang`, `wedge`, `fork`, `fork_silent`, `moved`,
`reject_tools`, `reject_filter`, or `no_ready`. `wedge` ignores abort and SIGTERM to exercise escalation.
`FAKE_OMP_DELAY_MS` delays the reply; `FAKE_OMP_READY_DELAY_MS` delays ready (startup abort tests use 5000);
`FAKE_OMP_SETTLE_DELAY_MS` delays settlement; `FAKE_OMP_REPLY_BYTES` sets assistant text size.
`FAKE_OMP_OWNERS_DIR` claims the owner lease on prompt. `FAKE_OMP_V2=1` enables full negotiation;
`partial` advertises v2 without frame limits and must stay on v1.

`FAKE_OMP_LOG` appends NDJSON records with `at_ms`: `start` carries pid, pgid, argv, cwd and selected env;
`in` / `out` carry a frame; `exit` carries the reason. Tests poll these records and status files as readiness
signals, then wait for worker and omp teardown before releasing their temporary directories.
