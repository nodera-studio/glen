# glen v0.2.0 tech debt audit (2026-11-03, main 582f77b)

Audited by 4 agents (3 Claude, 1 Codex) + lead synthesis. Findings only, no code edited. Codex scanner stalled (180s no activity) after running most scans; lead re-ran the quantitative counts. .claude/tech-debt/ was empty and the Mnemosyne memory_list tool was unavailable, so the registry was NOT updated (see Gaps). No P0 bugs found.

Sev: P0 bug, P1 confusing/risky, P2 cleanup. Effort S/M/L. Rec: FIX (before v0.2.0), ACCEPT (with reason), DEFER (past v0.2.0).

## Release-gating gaps
| # | Finding | Where | Sev | Eff | Rec |
|---|---|---|---|---|---|
| G1 | No RustSec scan run: cargo-audit/outdated/deny/license all missing (`cargo install --locked cargo-audit cargo-deny cargo-outdated cargo-license`). "No advisories" is unverified. | toolchain | P0-class gap | S | FIX: run `cargo audit` before tagging |
| G2 | Live-omp smokes unrun: rpc --resume does not claim lease before first prompt (Step 15), Step 12, Step 14, ACP E1-E3, W4 evidence | design.md §A17 ~587-593, 610 | P1 | S each, host only | FIX or ACCEPT with written reason |
| G3 | §A17 W4 line is checked [x] but cites `.claude/evidence/v020-wave-4-warm-worker/` which does not exist (evidence dir is git-ignored; waves 1,2,3,5 exist) | design.md:530, 602 | P1 | S | FIX: produce evidence or reword to "source read" |

## Pattern consistency (src/)
| # | Finding | file:line | Sev | Eff | Rec |
|---|---|---|---|---|---|
| F1 | `write_atomic` returns bare io::Error with no path; every v0.2 caller (warm publish/request_release, release.rs, route.rs, inbox.rs) prints "Permission denied (os error 13)" | src/glen_home.rs:187-197; callers rpc/warm.rs:387,483, commands/release.rs:168-184, ctl/route.rs:610, sandbox.rs:96 | P1 | S | FIX: wrap inside write_atomic like jobs::write_json (jobs/mod.rs:545) |
| F2 | `JobStore::status_lock` discards the path (`\|_, e\| GlenError::from(e)`) | src/jobs/mod.rs:375 | P1 | S | FIX |
| F3 | `release --all` aborts on first I/O error (`release_one(..)?`), losing rows already released | src/commands/release.rs:42 | P1 | S | FIX: collect per-worker errors as rows |
| F4 | Unreadable/torn warm registry silently dropped (`read_dir.flatten().flatten()`, `read_regular`->None); `unreadable` count discarded by release/ls; `release --all` says "no warm workers", exit 0 | src/rpc/warm.rs:431-453; release.rs:28; ls.rs:84 | P1 | S | FIX: surface count as warning |
| F5 | release_timeout hint always says `--force` even with --force; forced timeout then reported `cleaned` on next run without re-checking omp pgid (checkpoint-61, still true) | src/commands/release.rs:55-58,165-167,242 | P1 | S (hint) / M (recheck) | FIX hint; DEFER group re-check (needs design call) |
| F7 | MCP stdin read error treated as clean EOF (exit 0, no log); stdin thread spawn failure same | src/mcp/server.rs:81,89-91 | P2 | S | FIX (stderr line, exit 1) |
| F8 | MCP child errors lack context (program path not named; pipe-reader spawn failure yields empty capture silently) | src/mcp/dispatch.rs:519,395 | P2 | S | DEFER |
| F9 | omp spawn recipe (process_group + piped stdio) in 3 places; acp `guard()` mirrors `Ephemeral::open_session` | ctl/ephemeral.rs:40-60,320-345; ctl/probe.rs:113-130; commands/acp.rs:157 | P2 | M | DEFER (teardown-critical; already §A23 tech debt) |
| F10 | resolve_trusted + JobStore::new + paths::resolve preamble repeated; "set_model"<->"model" mapping hand-reversed | ctl/route.rs:322,517,339-343; commands/ctl.rs:33; send.rs:156; acp.rs:42 | P2 | S | DEFER |
| F11 | Dead items: ErrorCode::NotImplemented never constructed; warm::remove, clear_release test-only pub; unused `_cli` in release::run; `let _ = env` | error.rs:16; warm.rs:413,491,153; release.rs:21 | P2 | S | ACCEPT NotImplemented (reserved contract code, docs/json.md:39); FIX the rest opportunistically |
| F12 | `glen release` has no spinner/SIGINT handling; serial waits (N x 10s) | release.rs:177-190 | P2 | S | ACCEPT |
| F13 | env_digest hashes whole environment, so --warm may never hit across agent sessions; undocumented | warm.rs:87-103 | P2 | S | ACCEPT, add one doc line |
| F14 | design.md says 4s teardown bound; code is 3.5s/3.6s, and omp-rpc terminate adds 1s settle | design.md:589,694; warm.rs:36-39,1163 | P2 | S | FIX (docs) |
| F15 | send.rs warns with a /effort suggestion the bridge disables (w9 Codex review) | src/commands/send.rs:258 | P2 | S | verify; FIX if live |

PASS: no anyhow; omp parsing is Value/serde(default) based, deny_unknown_fields only on glen's own compat.json and facts.json; no write handle on a session .jsonl (all writes under GLEN_HOME, extension/skill installs, worker.log); --json/--human global, TTY decision in output/mod.rs:60-88; one Outcome feeds human and JSON; no println! in src, stdout via StdioWriter only; spinners gated on Human+stderr TTY; omp::paths::resolve is the single ~/.omp/PI_CONFIG_DIR resolver; no set_var anywhere; release signals only pid+start-token proven groups (release.rs:229-267); warm claim is compare-and-claim under status lock.

## Code complexity (lead scan; Codex partial: 121 non-test fns over 50 lines, longest `doctor::run` 418 lines at src/commands/doctor.rs:27)
- Large src files: tui/view.rs 3297, tui/update.rs 2706, rpc/worker.rs 2380, rpc/warm.rs 2262, commands/acp.rs 1407, tui/runtime.rs 1331, acp/client.rs 1319, live/client.rs 1249, output/envelope.rs 1207, mcp/dispatch.rs 1163, jobs/mod.rs 1139, mcp/server.rs 1104. Sizes include inline #[cfg(test)] modules. P1, effort L, DEFER (split warm.rs into registry/claim/teardown and doctor::run into per-check fns after v0.2.0; no defect attached).
- doctor::run 418 lines: P1, M, DEFER.
- #[allow]: 5 total, all with reason or clippy too_many_arguments (ctl/ephemeral.rs:238, jobs/wait.rs:137, commands/send.rs:145) and two commented in atomic_swap.rs. P2, ACCEPT.
- unsafe: 11 sites in 7 files (env.rs 3, liveness.rs 2, rest 1); SAFETY comments not individually verified. P2, DEFER.
- `let _ =` discards: warm.rs 6, mcp/dispatch.rs 6, worker.rs 3, live/client.rs 3, glen_home.rs 3 (teardown/cleanup best-effort; not individually verified). P2, ACCEPT unless a write result.
- Production unwrap/expect: raw counts are dominated by inline test modules; the pattern audit found none in focus files outside tests (line scan, not compile-checked). Not P0.
- eprintln! outside CLI layer (rpc/warm.rs:683, rpc/reply.rs:88,128,134, rpc/worker.rs:133,191, install_guard.rs:293,322): worker-side diagnostics to worker.log/stderr, deliberate. P2, ACCEPT.

## Markers
No TODO/FIXME/HACK/todo!()/unimplemented!/PARKED anywhere (XXX/TEMP hits are mktemp, RUNNER_TEMP, `\uXXXX`, NEXT_TEMP). Debt lives in design.md §A17/§A23 and checkpoints, not in code. Reviews: w9-cycle4 launcher exclusion lexical not physical (omp-engine.sh:100-139, P1, M, DEFER, harness only); five harness P2s (SBPL quoting, nocaseglob, smoke expectations, ladder thinking) DEFER; w8 O_NOFOLLOW lock open (P2, S, DEFER).

## Open items (checkpoint-68 "logged for W6"; note checkpoint-06.md is the v0.1 site wave, v0.2 text is in checkpoint-68.md)
| Item | State | Rec |
|---|---|---|
| EOF shutdown 2.1-2.9s with 600-request backlog (queued lines handled before Eof) | still true, mcp/server.rs:75-111, signal wins within ~1s | ACCEPT, note in §A22 |
| suite-lock.sh backgrounds the suite so tests inherit SIGINT ignored | product uses signal_hook, tests reset SIG_DFL (push.rs:600, omp_compat.rs:551) | resolved for known tests; one-line note |
| 4 mcp tests time out under flake-check | still true (see Test health T1) | FIX |
| flake-proof covers omp_compat/omp_surface/push/v2_push | yes (unfiltered run), but flake-proof.yml runs only on non-PR events or label | ACCEPT |
| checkpoint-61 forced timeout then `cleaned` | still true | see F5 |
| checkpoint-52 reply-source asymmetry, --after fallback, 1s wake budget, inbox --wait recollect cost | not re-verified | treat open, DEFER |
| §A17 W6 candidate: ACP facts missing from facts.json (25 facts, none ACP/PROTOCOL_VERSION) | still true | FIX (nightly cannot see ACP drift), effort S-M |
| §A17 W4: Linux network-on CODEX_SANDBOX and EROFS under bubblewrap | open, needs Linux host | DEFER |
| compat.json bump | tested/max_tested 18.14.0, matches omp-rpc tag and installed omp | none |

## Test health
| # | Finding | file:line | Sev | Eff | Rec |
|---|---|---|---|---|---|
| T1 | Four mcp tests time out under flake-check (CPU burners at 3n runnable threads, 10s held-job polls plus hard 1s/2s SLO asserts, each poll spawns /bin/ps): tests/mcp.rs:703 (cancel), 757 (eof/signals within 2s), 810 (stops reading stdout), 848 (closes stdout). Same shape in conformance/v2_mcp.rs:1435,1792,2067 and src/mcp/server.rs unit tests (781,792,854,868,926,1000,1101). Existing log: target/flake-check/run-1.log shows the shared 10s timeout. | tests/mcp.rs:485,497,676,687,695,821 | P1 | M | FIX: `common::scaled()` driven by GLEN_TEST_TIME_SCALE exported by flake-check.sh; replace repeated `ps` spawns with kill(pid,0); keep strict SLOs unloaded. Minimum: raise the 10s waits (S) |
| T2 | Sleep-based pipe-full readiness probe | tests/mcp.rs:821-824 | P2 | S | FIX with T1 |
| T3 | Absence checks that sleep (1.5s mcp.rs:721, 2.5s :987) can pass vacuously under load; wall-clock heavy files: watch.rs 93, live/client.rs 62, controls.rs 53, warm.rs 49 | tests/ | P2 | - | ACCEPT |
| T4 | Bare #[ignore] without reason | tests/lease.rs:6, site_demo.rs:21, json_contract.rs:1018 | P2 | S | FIX (cosmetic) |
| T5 | Duplicate helpers copy_tree (common/mod.rs:159, conformance/reader.rs:290) and per-file wait_for (mcp.rs:95, v2_mcp.rs:356, watch.rs:311, conformance/watch.rs:1028, fake_omp.rs:839) | | P2 | M | DEFER |
| T6 | Oversized conformance files (watch.rs 5440, agent.rs 4012, v2_mcp.rs 3786); 69.6k test lines | | P2 | L | ACCEPT |

PASS: no test touches a live ~/.omp; fixtures 96 KB, clean; fake_omp.rs is cohesive.

## Crate and toolchain currency
- Every direct dep is at newest except signal-hook 0.3.18 -> 0.4.6 (P1 by 0.x rule, S, DEFER, small Unix-only surface) and jiff 0.2.38 -> 0.2.39 (P2, ACCEPT). libc latest is a 1.0 alpha (ignore).
- Duplicate syn 2/3 via proc-macros only: ACCEPT. cargo build --locked: zero warnings, no deprecations.
- omp-rpc tag v18.14.0 = compat.json tested = installed omp. MSRV 1.89 (CI msrv job), toolchain pin 1.99.1: consistent.
- No deny.toml and no audit/deny CI step; only cargo-about at release.rs time. Lock has ~230 packages incl. 1 WTFPL, MPL-2.0, 1 null-license, BSL-1.0. P1, M, DEFER only if G1 done; otherwise FIX.
- CI actions: site-deploy (holds CLOUDFLARE_API_TOKEN) uses floating tags (checkout@v7, upload-artifact@v7, download-artifact@v8). P2, S, FIX for site-deploy; rest DEFER. ci.yml/flake-proof.yml mix @v4 and @v7. release.yml and omp-latest.yml are SHA-pinned.
- extension/glen has no package.json/engines: ACCEPT (single-file design).
- site/ npm: astro 7.4.4->7.4.5, tailwind 4.3.3->4.3.4, @lhci/cli 0.15->0.16, motion 12->14 (two majors), typescript 6->7, @types/node 24->26. DEFER all; patches in one npm update PR.

## Recommendations
1. Before tagging: G1 (cargo audit), G3 (evidence or reword), F1-F4 (path-bearing errors, release --all aggregation, unreadable-registry warning; all S), F5 hint, F14 docs, pin site-deploy actions, ACP facts in facts.json, and T1/T2 (flake-check green).
2. G2 live smokes: owner/host steps; run or accept with the reason written in §A17.
3. Defer: file splits (warm.rs, doctor::run, tui), spawn/guard dedup (F9), deny.toml, signal-hook 0.4, site majors, test helper consolidation.

## Gaps
- Codex scan incomplete; function-length and nesting tables are partial (121 fns over 50 lines, nesting not filtered of snapshots).
- Registry files in .claude/tech-debt/ not written and Mnemosyne not updated (memory_list/memory_store tools unavailable in this agent); the tables above are the DEBT items to import.
