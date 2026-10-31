# W9 fix cycle 4 — Claude re-review (e9b8373..37a52bd)

Scope: `.claude/omp/{omp-sandboxed.sh,model-ladder.sh}`, `.claude/scripts/{omp-engine.sh,omp-profile-setup.sh}`,
both smoke suites, `.claude/settings.json`, `.claude/skills/omp/SKILL.md`, `docs/json.md`, `src/cli.rs`,
`src/commands/send.rs`, `src/jobs/mod.rs`, `tests/cli.rs`, `tests/live.rs`.

Runs (this session):
- `omp-engine-smoke.sh` 5 consecutive runs: 524/524 each. `omp-profile-setup-smoke.sh`: 220/220.
- `cargo test --locked`: all suites green.
- `omp-engine.sh probe` against the real `glen-harness` profile under the narrowed policy: `status: ready`,
  model `cursor/gemini-4-argon` (no session created; agent.db not read by me).
- `sandbox-exec` experiments on a scratch profile tree under the session scratchpad, using the generated
  policy with only `process-exec*` relaxed so that coreutils could run (file and network rules unchanged).
- No real chat dispatch was run by this reviewer.

Verdict: **PASS** (0 HIGH, 3 MED, 8 LOW). None of the MEDs breaks the containment that fixes4 was about.
MED-1 and MED-3 are functional or guidance regressions. Fix them or record them as accepted.

## Fix-list verdicts

| # | Item | Verdict | Evidence |
|---|---|---|---|
| 1 | Launcher outside child-writable dirs | **partially closed** | Dispatch: closed. `dispatch_env` and `bounded_omp` use `launcher_dir_for` (`omp-engine.sh:49-58,379`); `$TMPDIR/glen-omp-engine-launcher` has no write grant; owner/mode re-checked (`:73-82,112-140`). Open: setup still places the launcher in the child's own run root (LOW-2), probe still copies into `PROBE_DIR` (LOW-3), and the guard is name-based. No smoke test checks refusal (LOW-5). The plan asked to run the launcher in place; it is copied instead (acceptable alternative). |
| 2 | Profile write allowlist + dir denies | **closed** (behaviour), smoke partial | Scratch-tree runs, all denied: rename agent dir, rename profile root, write `config.yml` and `mcp.json`, create `tools/x.ts`, `hooks/pre/x.ts`, `AGENTS.md`, `SYSTEM.md`, `settings.json`, `commands/x.md`, `rules/x.md`, `extensions/y.ts`, mkdir `extensions2`, `agentXdb`, `agent.db.bak`. Also denied: hard links into allowed names, and rename of `agent.db` onto `config.yml` or `SYSTEM.md`. Allowed: `agent.db`, `models.db-wal`, `sessions/*`. Regex escaping works for `.+()[]{}^$\|?*` (exp3). Residuals: MED-2 (symlink planting) and LOW-4 (`"` in the path). Smoke lacks most of these negatives (LOW-5). |
| 3 | Linux unavailable | **closed** | `probe_checks` refuses before any tool lookup (`omp-engine.sh:408-410`); `resolve_launcher` refuses independently (`:94`); SKILL.md Risks updated. |
| 4 | Narrow mach-lookup / process-exec / /Library | **closed**, with notes | No `mach-lookup` allow; `process-exec*` limited to the resolved binary (`omp-sandboxed.sh:98`, resolved path verified as a real Mach-O). The probe (`models cursor --json`, `config get`) completes under this policy. No explicit pasteboard/launchservicesd deny was added, and the comment's reason is wrong (LOW-6). `/Library/Caches/bun` does not exist on this host (LOW-6). |
| 5 | Flaky ancestor scan | **closed** | No `find` exit status; 5/5 smoke runs pass. The nocaseglob comment is wrong, but the behaviour matches omp's (LOW-6). |
| 6 | Outbound TCP 443 + DNS | **closed**, regression | `(remote tcp "*:443")` plus the mDNSResponder literal; residuals documented in SKILL.md. New regression: the harness's Mnemosyne MCP is unreachable (MED-1). |
| 7 | Deny-rule aliases | **closed** | `plugins`, `wt` and `skills` match `static aliases` in omp `commands/{plugin,worktree,skill}.ts`. `commands/settings.ts` is a settings-domain module, not a CLI command, so dropping it is correct. |
| 8 | Validate `--thinking` | **closed** | `src/cli.rs:213-227` matches omp `CLI_THINKING_LEVELS` (`tui/src/thinking.ts:9`, `catalog/src/effort.ts:11-18`); `tests/cli.rs:72` exit 2; docs list `minimal`. |
| 9 | `--thinking` on live route | **partially closed** | Warning emitted (`send.rs:255-260`) and tested (`tests/live.rs:495`), but its `/effort` hint does not work (MED-3). |
| 10 | Persist level in profile | **partially closed** | Suffix written (`omp-profile-setup.sh:260-261`); omp honours it when no explicit model or `--thinking` is given (`sdk.ts:2155`). The engine still gives an explicit `--model` a ladder level (LOW-1). |
| 11 | One shared ladder | **partially closed** | `.claude/omp/model-ladder.sh` is sourced by both scripts. The smoke asserts only `--thinking medium`, through the off-ladder fallback; no `high` assertion (LOW-5). |
| 12 | Old job-file test | **closed** | `src/jobs/mod.rs:487-506` passes. |
| 13 | DNS grants re-test | **closed per implementer evidence** | Commit 1438878 says a real send showed no DNS denials. Not independently re-run here. |
| 14 | `~/.omp/run` lease reads | **open** | No read grant and no recorded evidence (LOW-7). |

## Findings

### MED-1: TCP-443-only outbound cuts the harness off from its Mnemosyne MCP server
- File: `.claude/omp/omp-sandboxed.sh:171`, `.claude/omp/mcp.json:5`
- Premise: the omp reviewer can call `mcp__mnemosyne_memory_search` (`.claude/skills/omp/templates/review.md:27`, `debug.md:22`).
- Trace: the profile's MCP URL is `${MNEMOSYNE_MEMORY_URL:-http://localhost:8080/mcp}`. Before cycle 4, `(remote ip)` allowed it. Now only `*:443` is allowed. `MNEMOSYNE_MEMORY_URL` is unset in this environment, and something is listening on `127.0.0.1:8080` on this host: an unsandboxed `nc` connects, while under the generated policy `nc` gets `connectx … 8080 … Operation not permitted`.
- Failure: every dispatch silently loses Mnemosyne, and the reviewer can only print `Mnemosyne: unavailable`. The SKILL.md Risks section does not mention it.
- Fix: derive the port from `MNEMOSYNE_MEMORY_URL` (default 8080) in the launcher and add `(allow network-outbound (remote tcp "localhost:<port>"))`. SBPL hosts can only be `*` or `localhost`. Otherwise, document the loss in SKILL.md as an accepted residual.

### MED-2: The child can plant symlinks, or swap whole subtrees, in allowed profile paths that later unsandboxed omp runs follow
- File: `.claude/omp/omp-sandboxed.sh:137-151`
- Trace (scratch tree, generated policy): `ln -s <victim> agent/agent.db-journal`, `ln -s ../config.yml agent/sessions/sl` and `ln -s <victim> logs/omp.log` all succeed. So do `mv agent/sessions <run>/sess` followed by `ln -s <outside> agent/sessions`, and `mv logs <run>/logs`. Writes through these links fail inside the sandbox, because Seatbelt checks the resolved path. They succeed in later **unsandboxed** omp runs on this profile: setup's `profile` and `overlay` layers (`omp-profile-setup.sh:175-181`, e.g. `profile config get`, `profile toks`) and the owner's `omp --profile glen-harness login`. Those runs then append logs to, or create session files in, a directory the child chose. Planted `agent.db-*` sidecar links fail closed with the system SQLite: it reports "unable to open database file" (exp4), so that variant is a denial of service, not a write.
- Context: this is narrower than before cycle 4, when the whole agent dir was writable. It is a residual, not a regression, and it is the kind of escape item 2 targets.
- Fix:
  - Add `(deny file-write-create (vnode-type SYMLINK))`. Verified on the scratch tree: it blocks all three symlink plants while regular writes and mkdir in `sessions/` keep working.
  - Have the launcher pre-create `agent/sessions`, `logs`, `cache` and `run`, then deny `file-write-unlink` on those top-level literals so they cannot be renamed away.
  - Extend setup's `profile_path_guard` to refuse symlinks under them.

### MED-3: The `ignored_flag` hint tells agents to send `/effort <level>` as a message, which omp does not execute
- File: `src/commands/send.rs:258`, `docs/json.md:407`
- Trace: the live route delivers through the bridge's `pi.sendUserMessage` (`extension/glen/index.ts:190`). That reaches `AgentSession.sendUserMessage`, which calls `prompt(…, { expandPromptTemplates: false })` (`agent-session.ts:10302-10313`). Command handling is gated on `expandPromptTemplates` (`:8183`). `/effort` is a TUI built-in (`slash-commands/builtin-modes.ts:851`), not an extension command.
- Failure: an agent that follows the hint injects a literal "/effort high" user turn into someone's live session, and the model answers it. The level stays unchanged. The plan's `glen thinking` does not exist in this build either: it is guarded out of `skill/glen/SKILL.md:79`.
- Fix: say the level cannot be changed through glen on a live session, and that the session's user can change it in the omp TUI (`/effort`). Do not suggest sending it as a message.

### LOW-1: An explicit `--model` still gets a ladder thinking level per dispatch
- File: `.claude/scripts/omp-engine.sh:438-439,693`, `.claude/omp/model-ladder.sh:20`
- Trace: setup writes no suffix for an explicit model (`omp-profile-setup.sh:261`). But `thinking_level_for` returns `THINKING_LADDER[0]` (medium) for an off-ladder id, and the ladder level for an on-ladder id, and the engine always passes `--thinking`. That contradicts SKILL.md ("does not use the ladder") and the last bullet of item 10. Also, `${MODEL%%:*}` cuts at the first colon rather than at a valid trailing level.
- Fix: return empty for off-ladder or explicit ids, and pass `--thinking` only when it is non-empty (or when the persisted value carried a suffix). Strip only `:<level>` when `<level>` is in the CLI list.

### LOW-2: Setup still places the launcher inside the sandboxed child's writable run root
- File: `.claude/scripts/omp-profile-setup.sh:169,173`, `.claude/scripts/omp-engine.sh:100-111`
- Trace: `launcher "$ENGINE_ROOT" "$cwd"`, then `OMP_SANDBOX_RUN_ROOT="$cwd"`. The guard only rejects `*/glen-omp-engine/run-*` and `$RUN_DIR`. `omp-engine.sh launcher <repo> <scratch>/other-run-root` returned a launcher path with rc 0.
- Impact: none today, because setup uses each launcher once and the bash launcher has already `exec`ed into `sandbox-exec` before the child can write. But this breaks the item-1 invariant, and a later reuse would make it exploitable.
- Fix: have setup call `launcher` with `launcher_dir_for` (or make DEST optional and default to it).

### LOW-3: A dead launcher copy in `PROBE_DIR`, and unbounded launcher accumulation
- File: `.claude/scripts/omp-engine.sh:420,422,429,128`
- Trace: `resolve_launcher "$ENGINE_ROOT" "$PROBE_DIR"` still runs, which contradicts commit a93c791 ("never RUN_DIR/PROBE_DIR"). `bounded_omp` runs inside `$(…)`, so its `LAUNCHER_BIN` never reaches the parent, and each call mints a fresh `mktemp` file. One real probe printed three "main launcher unavailable" warnings and left 2 files in `$TMPDIR/glen-omp-engine-launcher`. Nothing prunes that directory.
- Fix: resolve once in the parent shell into `launcher_dir_for` before `bounded_omp` runs, drop line 420, and prune or reuse files (for example a content-hash name).

### LOW-4: `sbpl_regex_literal` produces an unparsable profile for a path containing `"`
- File: `.claude/omp/omp-sandboxed.sh:56-58`
- Trace: `#"…"` is a raw literal, so `\"` ends it. An agent dir `…/quo"te/…` made `sandbox-exec` fail with "unbound variable: te/glen-harness/agent/". The failure is closed (omp does not run), so nothing is bypassed.
- Fix: `die` on `"` the same way as on a newline.

### LOW-5: Smoke coverage gaps for items 1, 2 and 11
- File: `.claude/scripts/tests/omp-engine-smoke.sh:251-257,854-858,1134`
- Missing: a "launcher in RUN_DIR is refused" case (`launcher SRC <run-dir>` exits 4, checked manually); runtime negatives for renaming the agent dir and profile root, and for creating `tools/x.ts`, `hooks/pre/x.ts`, `AGENTS.md`, `SYSTEM.md`, `settings.json`, `commands/x.md` and `rules/x.md` (all checked manually above); a `--thinking high` assertion. The only `--thinking medium` assertion comes from the off-ladder fallback, so it does not show that an on-ladder id maps to its own level. Lines 183-185 also count an occurrence inside a comment, which is brittle.

### LOW-6: Comments that contradict verified behaviour
- `omp-sandboxed.sh:180-182`: "SBPL global-name has no wildcard" is wrong. Both `(global-name-prefix "com.apple.pasteboard.")` and `(global-name-regex …)` parse and run. Item 4 asked for an explicit deny.
- `omp-sandboxed.sh:72-74`: still says new filenames are "covered by the broad write grant", which no longer exists.
- `omp-sandboxed.sh:115`: `/Library/Caches/bun` does not exist on this host (Bun's cache is per-user), so the grant is dead.
- `omp-engine.sh:336-342`: `"$ancestor"/AGENTS.md` has no glob metacharacter, so nocaseglob and nullglob never apply. The check finds case variants only because `[ -f ]` runs on a case-insensitive filesystem (scratch test: the empty dir still yields 1 element). This matches omp's exact-name lookup (`discovery/helpers.ts:747`), so the behaviour is fine and the comment is wrong.

### LOW-7: Item 14 is open: no read grant for `~/.omp/run`
- File: `.claude/omp/omp-sandboxed.sh:100-131,147`
- Trace: the lease `flock` still works, because opening it O_RDWR under the write grant succeeded. Reading lease or daemon JSON is denied (`cat` gets EPERM in the scratch run). omp treats a lease error as "owned" (`session-storage.ts:1308-1310`). No deny-log evidence was recorded.
- Fix: record the deny-log result, or add `(subpath "$home_sb/.omp/run")` to `file-read*`.

### LOW-8: The ladder is sourced before the EXIT trap
- File: `.claude/scripts/omp-engine.sh:17` vs `:287`
- Trace: if `model-ladder.sh` cannot be resolved, `set -e` exits before `finish` is registered, so `probe` prints no JSON result line, which breaks its output contract.
- Fix: source it after `trap finish EXIT`, or guard it with `|| { printf '{…"status":"failed"…}'; exit 4; }`.
