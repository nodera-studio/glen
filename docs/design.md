# glen v0.1.0 design

Status: v0.1.0 design, written in W0. Target omp version: **18.14.0**.

glen uses three channels: a read-only disk reader discovers sessions and reads transcripts; an RPC driver runs omp in a detached worker for each job; a bundled omp extension routes requests into live sessions and carries replies back. The channels share a single CLI and versioned output contract.

## §A1 Scope and non-goals
- v0.1.0 supports **macOS and Linux**. Windows is out of scope: the lease probes and unix sockets are Unix-only, and that code is `#[cfg(unix)]`.
- glen never writes, renames or deletes omp session files. It never takes omp's `.<file>.jsonl.lock.os` append gate, and it never creates files inside omp's directories. The exception is `install-extension`, which writes `<agent_dir>/extensions/glen/` on explicit request.
- No MCP server in v0.1.0 (an optional extra later). No network access.
- Not listed in v0.1.0: archived sessions (`archive/sessions/*.jsonl.gz`), subagent transcripts (nested `<stem>/*.jsonl`) and collab replicas.

## §A2 omp surfaces (v18.14.0)
CA = `packages/coding-agent/src`; references are pinned to v18.14.0.

| Surface | Source at v18.14.0 | Contract |
|---|---|---|
| Session JSONL v3 | `CA/session/session-entries.ts`, `docs/session.md` | Header plus append-only tree entries |
| Title slot | `CA/session/session-title-slot.ts` | First physical line is 256 bytes |
| Leaf | `CA/session/session-entries.ts:360-383` | Last entry's activeLeafId, else its id |
| Continuations | `CA/session/session-listing.ts:892` | `findSessionContinuation` follows continuationOf |
| Owner directory | `packages/utils/src/dirs.ts:1150` | Shared across profiles |
| Owner lease name | `CA/session/session-storage.ts:618-639` | `tryAcquireSessionLease` uses sanitised session id |
| Owner lease suffix | `packages/utils/src/file-lock.ts:31-33, 43-45` | `getLockPath` appends `.lock` to the lexical `path.resolve` string |
| Owner lease acquisition | `CA/session/session-storage.ts:1295-1326` | Claimed on first write |
| Linux abstract-socket name | `crates/pi-natives/src/file_lock/mod.rs:32-40`, `crates/pi-natives/src/file_lock/linux.rs:14-22` | XXH64 seeds and datagram bind; decision `47efa1c2` |
| RPC wire and official Rust client | `CA/modes/rpc/wire/rpc-wire.schema.json`, `sdk/rust/omp-rpc` | Prompt acknowledgements, result and settled frames |
| Extensions | `CA/session/agent-session-types.ts:513-517`, `CA/session/agent-session.ts:10261-10300` | sendUserMessage deliverAs, registerTool, session_start/session_switch |
| Duplicate host tool rejection | `CA/modes/rpc/rpc-mode.ts:2365-2369` | One glen_reply provider per process |
| Directories | `packages/utils/src/dirs.ts` | §A12 |

## §A3 The three channels
**Disk reader (W1)**
- **Discovery.** Buckets are the immediate subdirectories of `sessions_dir`. Session files are `<bucket>/*.jsonl` with a valid header. Skip dotfiles (`.lock.os`, `.*.tmp`) and everything inside subdirectories.
- **Bucket for a cwd.** Port `getDefaultSessionDirName` (`CA/session/session-paths.ts`). Canonicalise the cwd. The temp root is checked first (`-tmp-<rel>`), then home (`-<rel>`), else `--<abs>--`. Separators `/ \ :` become `-`.
- **Listing** reads a 4 KiB head and a 32 KiB tail per file, as omp does. `show`/`tail` read the whole file.
- **Parsing:**
  - An optional title slot on line 1 is folded into the header.
  - The first logical line must be `type:"session"` with a string `id`; otherwise the file is not a session.
  - Malformed lines are skipped and counted (`skipped_lines` in JSON).
  - Unknown entry types are kept as opaque tree nodes (id/parentId), so the parent chain stays intact. They are never rendered except with `--raw`.
  - Unknown fields are ignored. Unknown content blocks render as `[<type>]` in human mode and pass through with `--raw`.
- **Leaf.** If the last entry has `activeLeafId`, that is the leaf (`null` = empty). Otherwise the leaf is the last entry's id. A captured leaf id that no longer exists falls back to the newest entry that selects itself.
- **Active branch.** Walk `parentId` from the leaf, stopping on a repeated id, then reverse.
- **Rendering:**
  - Shown: `message` by role (user, assistant, toolResult, developer, bashExecution, pythonExecution, custom, fileMention); `custom_message` with `display:true`; `branch_summary`; `compaction` and `reset_boundary` as dividers; `model_change` and `thinking_level_change` as muted meta lines.
  - Thinking is hidden by default (`--thinking` shows it). Tool calls are collapsed by default (`--tools`).
  - Everything else is hidden unless `--raw`.
- **Continuations.** Port `findSessionContinuation`: follow files whose header `continuationOf` names the session, recursively, and take the copy that holds the newest turn. JSON reports `continued_from: [ids]`. Never follow a fork (`parentSession` without `continuationOf`).
- **Liveness** is a read-only probe of `<session_owners_dir>/<name>.lock`:
  - The `.lock` suffix is appended by `file-lock.ts` `getLockPath`, not in `session-storage.ts`; see §A2.
  - `<name>` is the header's session id when it matches `^[A-Za-z0-9_-]{1,128}$`, which every uuid does. Any other id is named `h-` + `Bun.hash.wyhash(id)` in 16 hex digits. glen does not port wyhash in v0.1.0, so such ids probe as `unknown`.
  - The path is the lexical absolute string omp would produce (`path.resolve`: no symlink resolution, no realpath).
  - macOS: open the file *without* `O_CREAT`. ENOENT → not live. `flock(LOCK_EX|LOCK_NB)`: EWOULDBLOCK → live; success → unlock at once → not live.
  - Linux: bind the abstract DATAGRAM socket `omp-file-lock-` + XXH64(path, seed 0x4f4d502d4c4f434b) hex16 + XXH64(path, seed 0x50492d46494c454c) hex16, where path is the UTF-8 string above. The hex is lowercase and zero-padded. EADDRINUSE → live; success → close → not live. This is a bind, never a connect, and never a stream socket.
  - Any other error → `unknown`, including EACCES/EPERM: a permission error is never evidence that the lease is free.
- **ls status columns.** `live`/`idle`/`unknown`, plus the last-turn state `complete|interrupted|aborted|error|pending|unknown`, ported from `session-listing.ts`.

**RPC driver (W2)**
- `omp-rpc` from git, tag `v18.14.0`.
- **Per-job detached worker.** `glen send` writes the job, then spawns the hidden internal `glen __worker <job-id>`: `setsid`, stdin null, stdout/stderr to `jobs/<id>/worker.log`. This happens before any RPC client exists. The foreground `send`/`wait` only read `status.json`.
- **The worker:**
  1. Takes the glen per-session lock `run/locks/<sessionId>.lock` (blocking flock; status `queued` while waiting). This serialises glen requests per session.
  2. Probes the owner lease immediately before spawning.
  3. Spawns `omp --mode rpc --resume <file>`, with `--profile` passed through and env `GLEN_MANAGED=1`. For `@new`: no `--resume`, cwd = `--cwd`.
  4. Calls `set_host_tools([glen_reply])`, then `prompt`.
  5. On `PromptAck` → status `accepted`.
  6. Completion = the `prompt_result` frame AND `session_settled`. `prompt_result.sessionSettled` may be false; keep waiting.
  7. Writes `result.json` and a terminal status, then tears down omp's process group.
- **Fork detection.** Compare the session id/file before and after (`get_state`), and watch for persistence `notice`/warning frames. A change → `status.forked = {from, to}` → exit 11.
- **Tests** use a fake RPC peer through `GLEN_OMP_BIN`.

**Live route (W3)**
- Bundled TypeScript extension at `extension/glen/index.ts` in the repo, embedded with `include_str!`. `glen install-extension` writes it to `<agent_dir>/extensions/glen/index.ts` (profile-aware). The written file bakes in the glen version and the resolved GLEN_HOME; `GLEN_HOME` set at omp runtime overrides it.
- **On `session_start`:** if `GLEN_MANAGED=1`, do nothing. Otherwise register the `glen_reply` tool, bind `$GLEN_HOME/run/live/<pid>.sock`, and write `run/live/<sessionId>.json` (0600): `{v:1, pid, session_id, session_file, socket, ext_version, omp_version, started_at}`.
- **Identity changes:** follow `session_switch` and persistence continuations; on a change, re-key the registry (write the new file, remove the old).
- **Shutdown:** close the socket and remove both files within 2 s. Stale sockets are recovered on start.
- **Protocol:** NDJSON over the unix socket, one connection per request.
  - `{"v":1,"op":"hello"}` → `{ok, session_id, busy, ext_version}`.
  - `{"v":1,"op":"send","request_id","job_id","text","deliver_as":"auto|steer|follow_up"}` → `{"event":"ack","delivered_as"}`. An ack is not delivery and not completion.
  - Then events stream until `{"event":"settled","status":"completed|aborted|error"}`, including `{"event":"reply","text","final"}` and `{"event":"session_changed","from","to"}`.
- **deliverAs normalisation.** `auto`: idle → prompt (deliverAs omitted), busy → `followUp`. `steer`: busy → steer, idle → prompt. `follow-up`: busy → followUp, idle → prompt. Explicit steer/followUp queue even when idle, so they are never sent while idle.
- `pi.sendUserMessage` returns void. Completion comes only from observed events.

## §A4 Routing (`glen send`)
1. Resolve the target (§A8) and follow continuations. `@new` → RPC with a fresh session.
2. **Live endpoint.** If `run/live/<sessionId>.json` exists, connect and send `hello`. A matching `session_id` → live route. A dead pid, ECONNREFUSED or a missing socket → delete the registry (best effort) and continue.
3. **Owner lease not held** → RPC route. The worker re-probes immediately before prompting; if it is held then, the job fails `no_route`.
4. **Lease held and no endpoint** → refuse: exit 6 `no_route`, hint "run `glen install-extension`, then restart that omp session".
5. **Probe result unknown** → refuse (`no_route`).

**Residual race, stated plainly.** omp claims the owner lease on its first write, so probe → spawn → prompt is not atomic. If a TUI opens the same session between glen's probe and the RPC child's first write, one of the two writers continues in a sibling file. glen narrows the window (endpoint first, probe immediately before prompting, a per-session glen lock) and detects a fork afterwards (exit 11 `forked`, reporting both ids). It cannot prevent it. On macOS the probe itself holds the flock for microseconds; that is the same exposure `omp gc` has.

## §A5 Reply channel (`glen_reply`)
- **Tool** `glen_reply`, params `{text: string, final?: boolean}`. Description: "Send a message back to the agent that contacted you through glen (Claude Code or Codex). Use it to answer a glen request or report progress." It returns `"delivered"`.
- **Exactly one provider per omp process.** omp rejects a host tool whose name an extension already registered.
  - glen's RPC children run with `GLEN_MANAGED=1`; the extension skips its tool and socket there, and the worker supplies a host tool via `set_host_tools`.
  - TUI and print sessions with the extension installed use the extension's tool.
- **Delivery:**
  - RPC: the worker records replies in `result.json.replies` and appends them to the inbox.
  - Live: the extension streams a `reply` event to a connected worker (when a glen request is active) and appends to `inbox/<sessionId>.ndjson`: `{v:1, id, session_id, job_id|null, request_id|null, text, final, at}`.
- **Reading:** `glen inbox` shows unread messages, tracked by a per-session byte-offset cursor in `inbox/.read/<sessionId>`, and marks them read unless `--peek`. `--follow` streams NDJSON.

## §A6 glen state (`GLEN_HOME`)
```
$GLEN_HOME            default ~/.glen; must be absolute; leading ~ expanded; relative → invalid_env (exit 2)
├── run/live/<sessionId>.json, run/live/<pid>.sock    W3 live endpoints
├── run/locks/<sessionId>.lock                        W2 per-session request lock
├── jobs/<jobId>/{job.json,status.json,result.json,worker.log}   W2
├── inbox/<sessionId>.ndjson, inbox/.read/<sessionId>             W3
└── out/<unix-ms>-<command>-<pid>.{json,txt}                      W0 spilled output
```
- **Modes:** directories glen creates are 0700, files 0600. glen never chmods a GLEN_HOME that already exists; doctor warns about group/other bits.
- **Atomic writes:** a temp file in the same directory, then rename.
- **Socket paths:** `sun_path` holds at most 104 bytes on macOS. A socket path that is too long is refused, with a hint to shorten GLEN_HOME.
- **Retention:**
  - `out/` files older than 7 days are pruned on every spill.
  - Terminal jobs older than 7 days are pruned when a new job is created.
  - Inbox files are kept.

## §A7 Jobs
- **Id:** a glen-generated uuidv7. **Handle:** `j` + the last 7 hex.
- **`job.json`:** `{v, id, session_id|null, session_file|null, route: "rpc"|"live"|"new", deliver_as, text, cwd, created_at}`.
- **`status.json`:** `{v, status, updated_at, worker_pid, request_id?, actual_session_id?, forked?: {from, to}, error?: {code, message}}`.
- **`result.json`:** `{v, assistant_text, replies[], stop_reason, settled_at}`.
- **Statuses:** `queued → accepted → completed | aborted | failed`, and `queued|accepted → lost`.
- **`lost`** is detected by readers: a non-terminal status, plus the worker pid not alive (`kill(pid,0)` = ESRCH), plus `status.json` older than 2 s. The reader then persists `lost`.
- **Exit mapping for `send`/`wait`:**
  - completed → 0, or 11 if forked;
  - aborted, failed or lost → 7 `job_failed` (`details.status`);
  - still `queued` or `accepted` when the wait bound elapses → 10 (`ok:true`, `data.status:"running"`, `data.job`).
  - SIGINT exits 130 (shell convention), and the worker keeps running.

## §A8 Handles
- **Session handle** = the **last 7 hex digits** of the dash-free session id.
  - omp ids are uuidv7, created with `Bun.randomUUIDv7()`. The leading 48 bits are a millisecond timestamp, so sessions created minutes apart share 6–8 leading hex digits. Fixture sessions A and S share `01a0f6b`, so prefix handles would be long and would keep growing.
  - Bun keeps the final 8 bytes random. A tail handle is therefore fixed-length, independent of the other sessions (stable), and unique with probability about 1 − n²/2²⁹ (n = 5,000 → about a 5% chance of any collision).
  - On a collision, `ls` shows the shortest unique longer tail for the colliding sessions only.
  - Legacy non-uuid ids use the shortest unique prefix, at least 7 chars.
- **Job handle** = `j` + the last 7 hex of the job id. A hex id never contains `j`, so job and session handles cannot be confused. A `j…` argument where a session is expected → `usage`, with a hint to use `glen wait`.
- **Resolving `<session>` arguments:**
  1. `@latest` = the newest session (by mtime) in the bucket of the cwd, or of `--cwd`.
  2. A path (contains `/` or ends `.jsonl`) = that file.
  3. Otherwise match case-insensitively over all sessions in `sessions_dir`. Take the union of: the full id; a hex suffix of the dash-free id (at least 4 hex); an omp-style prefix of the id or file name (at least 8 chars).
     - One match → resolved.
     - Several → exit 4 `ambiguous`, with the candidates in `error.details.candidates`.
     - Two files whose headers carry the same session id (a copied file) are two candidates even for a full-id match. glen never picks one silently, because omp keys the lease by id and both copies share it.
     - None → exit 3 `not_found`.
  4. Then follow continuations.
- `@new` is valid only for `glen send`.

## §A9 Commands (v0.1.0)
| Command | Purpose | Wave | Visible in W0 |
|---|---|---|---|
| `glen ls [--all] [--cwd DIR] [-n N] [--live]` | List sessions for the cwd bucket (`--all`: every bucket). Columns: handle, title, status, last turn, updated ("3m ago"), model; plus cwd with `--all` | W1 | hidden |
| `glen show <session> [--thinking] [--tools collapsed\|full\|none] [--raw]` | Active-branch transcript; `--raw` = entries as JSON | W1 | hidden |
| `glen tail <session> [-n N] [-f] [--thinking]` | Last N transcript items; `-f` follows (NDJSON in JSON mode) | W1 | hidden |
| `glen send <session\|@latest\|@new> <message\|-> [--as auto\|steer\|follow-up] [--wait DUR\|--no-wait] [--cwd DIR]` | Send through the live route or RPC, then wait up to the bound (default 90s) | W2 (RPC), W3 (live) | hidden |
| `glen wait <job> [--timeout DUR]` | Keep waiting for a job (default 90s) | W2 | hidden |
| `glen jobs [--session S] [--active] [-n N]` | List glen's jobs | W2 | hidden |
| `glen inbox [--session S] [--peek] [--all] [-f]` | Messages omp sent back with `glen_reply` | W3 | hidden |
| `glen watch [<session>]` | Full-screen live view (ratatui) | W5 | hidden |
| `glen install-extension [--dry-run] [--force] [--uninstall]` | Install the bundled omp extension for the effective agent dir/profile | W3 | hidden |
| `glen install-skill [--claude] [--codex] [--project] [--dry-run] [--force] [--uninstall]` | Write SKILL.md for Claude Code and/or Codex | W4 | hidden |
| `glen doctor` | omp binary and version, resolved paths, glen home (later: extension, skill, endpoints) | W0 (extended W3/W4) | visible |
| `glen completions <bash\|zsh\|fish\|elvish\|powershell>` | Shell completion script (raw) | W0 | visible |
- **Global flags:** `--json`, `--human` (mutually exclusive), `--color auto|always|never`, `--profile NAME`, `-h`, `-V`.
- **Defaults and aliases:** `--color` defaults to `auto`; `--profile` overrides `OMP_PROFILE` and `PI_PROFILE`. `ls` and `jobs` use `-n/--limit` (default 20); `tail` uses `-n/--lines` (default 10). `tail` and `inbox` accept `-f/--follow`. `show --tools` defaults to `collapsed`; `send --as` defaults to `auto`; `send --wait` and `wait --timeout` default to `90s`. `-h/--help` and `-V/--version` are available on every command. W0's hidden commands return `not_implemented` (exit 8).
- **Durations:** `<n>ms|s|m|h` or bare seconds, at most 24h. `--wait 0` equals `--no-wait`.
- **Internal:** `glen __worker <job-id>` (W2), always hidden.
- **Considered and rejected:**
  - a `glen reply` CLI: the reply path is the `glen_reply` tool, and an omp Bash call has no job context;
  - a separate `glen steer` verb: folded into `send --as`;
  - a `--new` flag: an optional first positional is ambiguous, so it became `@new`;
  - an MCP server: not needed for v0.1.0.

## §A10 Output contract
- **Modes.** `--json` → JSON; `--human` → human; otherwise human when stdout is a TTY and JSON when it is not. Raw commands (`completions`) print raw unless `--json` is given. `--help` and `--version` are always text.
- **Streams.** stdout carries only the result. stderr carries diagnostics, warnings (human mode) and spinners. In JSON mode stderr stays empty except for panics.
- **Envelope** (`v` = contract version 1; fields are append-only; keys are snake_case; timestamps are RFC 3339 UTC; durations are `*_ms` integers; JSON paths are absolute):

| Document | JSON contract |
|---|---|
| Success | `{"v":1,"ok":true,"data":{…},"warnings":[{"code","message"}]?}` |
| Error | `{"v":1,"ok":false,"error":{"code","message","hint"?,"details"?},"warnings"?}` |
| Spilled | `{"v":1,"ok":true,"spilled":{"path","bytes","format":"json"|"text"}}` |

  - Pretty-printed on a TTY, compact otherwise.
  - `ok:true` holds for exits 0, 10 and 11.
- **W0 doctor data:** `data.glen` contains `version`, `home` and `home_exists`; `data.omp` contains `bin` (path or null), `version` (string or null) and `tested_version` (`18.14.0`). `data.paths` contains the §A12 resolved paths, `profile` (name or null), `agent_dir_source` (`default|profile|env`) and `sessions_dir_source` (`agent_dir|xdg`). `data.checks` is an ordered array of `{name,status,detail}`, with status `ok|warn|fail|info`. The checks are `omp_binary`, `omp_version`, `profile`, `agent_dir`, `sessions_dir`, `glen_home`. An untested version succeeds with warning `omp_version_untested`. A failed doctor returns `omp_unavailable`, with the same data under `error.details.report`, and its human table before the error line. Missing directories are reported without creating them.
- **Explicit JSON completions:** `data.shell` names the shell and `data.script` contains its raw script. Default redirected completions remain raw.
- **Usage errors in JSON mode** become an envelope with code `usage` on stdout, exit 2.
- **NDJSON streams** (`tail -f`, `inbox -f`): one compact object per line, `{"v":1,"event":"<kind>",…}`. The last line is `{"v":1,"event":"end","ok":true}` or an error envelope. Streams are never spilled.
- **Spill.** Single-document output on a non-TTY stdout that exceeds `GLEN_MAX_OUTPUT` (default 24000 bytes, below Claude Code's 30000-char Bash cap; `0` disables; an invalid value uses the default plus warning `invalid_env`) is written to `$GLEN_HOME/out/…` (0600, no ANSI). glen prints the spilled envelope instead, or in human mode `output is <N> KB; full output in <path>`.
- **Colour** (per stream; JSON is never coloured), in precedence order:
  1. `--color always|never`
  2. `NO_COLOR` with any non-empty literal value (including whitespace) → off
  3. `CLICOLOR_FORCE` with any non-empty literal value other than `"0"` (including whitespace) → on
  4. `CLICOLOR=0` → off
  5. TTY and (`TERM` set and not `dumb`, or `CLICOLOR` set and not `0`, or `CI` set)
  - Every write goes through `anstream::AutoStream`.
- **Depth:** `COLORTERM` truecolor/24bit → 24-bit; `TERM` containing `256color` → 256; otherwise 16.
- **Theme:** `GLEN_THEME=dark|light`, then the last semicolon-separated integer in `COLORFGBG` (0–6 or 8 → dark; 7 or 9–15 → light), then dark.
- **Palette.** Foreground only; body text keeps the terminal's default colour. Sourced from the Nodex redesign content tone (research `93cec9de` rev 2):

| Token | Light hex | Dark hex | xterm L/D | ANSI-16 |
|---|---|---|---|---|
| muted | #505050 | #B4B5BD | 239 / 250 | default + dim |
| border | #DADADA | #45474D | 253 / 238 | bright black |
| accent (user role) | #5E6AD2 | #7170FF | 62 / 63 | blue |
| accent_2 (assistant role) | #12A594 | #12A594 | 36 / 36 | cyan |
| success | #007F3D | #00CA50 | 29 / 41 | green |
| warning | #A66300 | #FF9300 | 130 / 208 | yellow |
| danger | #DC2626 | #FF565F | 160 / 203 | red |
| focus | #4069FD | #94ADFF | 12 / 111 | bright blue |
| tool role | = muted | = muted | | |
| (site only) bg / surface / text | #FFFFFF / #F8F8F8 / #131313 | #191A1C / #222326 / #F4F4F6 | | |

- **Width:** `COLUMNS` (20–1000), then the terminal width when stdout is a TTY, otherwise unbounded.
- **Tables:** no borders, 2-space gutters, header bold + muted, no trailing whitespace.
- **Spinner:** stderr, human mode with a TTY stderr only, braille frames at 80 ms, cleared when done.
- **Glyphs:** ✓ ok, ! warn, ✗ fail, ● live, ○ idle, · info.
- **Paths:** `~/…` in human output, absolute in JSON.
- **Untrusted text.** Every string glen did not author (paths, env values, session titles, cwds, previews, model output) passes through `render::sanitize` before it is painted in human mode. Control bytes are shown escaped and never reach the terminal raw. JSON relies on serde's escaping.
- **Write failures.** A failed or closed stdout ends the command with exit 1 and no further stdout output. glen never panics on EPIPE and never writes a second envelope.
- **Lazy state.** `--help`, `--version`, `doctor` and path resolution never create `GLEN_HOME`. Directories are created on first write (spill in W0; jobs, inbox and live endpoints later).

## §A11 Exit codes
| Exit | `error.code` / status | Meaning |
|---|---|---|
| 0 | — | success (`ok:true`) |
| 1 | `internal`, `io` | unexpected failure |
| 2 | `usage`, `invalid_env` | bad flags or arguments; bad env value (`OMP_PROFILE`, `PI_PROFILE`, `GLEN_HOME`, missing `HOME`) |
| 3 | `not_found` | session, job or handle not found |
| 4 | `ambiguous` | handle matches several; candidates in `details` |
| 5 | `omp_unavailable` | omp missing, unrunnable or version unparseable |
| 6 | `no_route` | live owner without a glen endpoint, or probe unknown; refusing to fork |
| 7 | `job_failed` | omp reported aborted/error, or the worker was lost (`details.status`) |
| 8 | `not_implemented` | subcommand declared but not in this build |
| 10 | (ok) `running` | wait bound elapsed; job continues; use `glen wait <job>` |
| 11 | (ok) `forked` | omp moved the session to a continuation; use the new handle |
| 130 | — | interrupted (SIGINT); a detached job keeps running |

## §A12 omp path resolution (port of `packages/utils/src/dirs.ts` @ v18.14.0)
1. **home** = `$HOME` if non-empty, else the passwd entry. If neither exists → `invalid_env`.
2. **Base config root** = home joined with `PI_CONFIG_DIR` (if non-empty, else `.omp`), using Node `path.join` semantics: an absolute value lands *under* home, and `..` is normalised lexically. The base is profile-independent.
3. **Profile** = `--profile`, else `OMP_PROFILE` if set (even an empty value selects default), else `PI_PROFILE`.
   - Trimmed. Empty or `default` → the default profile.
   - Valid: `^[a-z0-9][a-z0-9._-]{0,63}$`, not `.` or `..`, not ending in `.`, not a Windows reserved basename.
   - Invalid flag → `usage`; invalid env → `invalid_env`.
4. **Config root** = base, or `base/profiles/<p>`.
5. **Agent dir:**
   - named profile → `config_root/agent`, and `PI_CODING_AGENT_DIR` is ignored;
   - default profile → `PI_CODING_AGENT_DIR` (non-empty, resolved against the cwd), unless it equals `base/profiles/<PI_PROFILE>/agent` (a profile-derived value, ignored);
   - otherwise `config_root/agent`.
6. **XDG** (Linux and macOS), only when the agent dir is the profile's default:
   - `XDG_DATA_HOME` non-empty and `<XDG_DATA_HOME>/omp` exists (named profile: `<XDG_DATA_HOME>/omp/profiles/<p>` exists) → sessions = `<that>/sessions` (flattened, no `agent/`);
   - otherwise `agent_dir/sessions`.
7. **Session owners** = `<XDG_STATE_HOME>/omp/run/session-owners` if `XDG_STATE_HOME` is non-empty and `<XDG_STATE_HOME>/omp` exists, else `base/run/session-owners`. This is shared across profiles and ignores the agent-dir override.
8. **Extensions** = `agent_dir/extensions` (no XDG redirect).
9. **Known gap.** omp fills *unset* `PI_CODING_AGENT_DIR`/`XDG_*_HOME` from `.env` files (cwd `.env`, `<agent_dir>/.env`, `<config_root>/.env`, `~/.env`; `utils/src/env.ts:289-316`). glen reads only its own environment. Doctor (W4) warns when those files define these keys.

- **Environment handling difference.** glen drops non-UTF-8 environment keys/values and treats whitespace-only path values as unset; omp uses JavaScript strings and treats whitespace-only values as set. Profile names are still trimmed. `NO_COLOR` and `CLICOLOR_FORCE` use literal values as specified in §A10.

- **omp executable:** `GLEN_OMP_BIN` (non-empty) accepts an absolute file path or a bare executable name searched on PATH; otherwise glen searches PATH for executable `omp`. `doctor` runs `--version` with a 10 s deadline. The first stdout line must be `n.n.n`, optionally prefixed by `omp/`. Failure, timeout or unparseable output is `omp_unavailable`.

## §A13 How Claude Code and Codex use glen
- `glen install-skill` (W4) writes two variants:
  - Claude Code: `~/.claude/skills/glen/SKILL.md`, with frontmatter `name`, `description` and `allowed-tools` pre-approving `glen` Bash calls. Verify the exact rule syntax against current Claude Code docs in W4.
  - Codex: `~/.agents/skills/glen/SKILL.md`, with `name` and `description` only. Codex ignores `allowed-tools`, and `~/.codex/skills` is legacy.
  - `--project` writes `./.claude/skills/glen/` and `./.agents/skills/glen/` instead.
- **Agent loop:** `glen ls` → `glen show <h>` / `glen tail <h>` → `glen send <h> "…"`. Then branch on the exit code:
  - 0 → the reply is in `data`;
  - 10 → `glen wait <job>`;
  - 6 → ask the user to install the extension and restart omp;
  - 11 → use the new handle.
  - Afterwards, `glen inbox`.
- Agents always get JSON (non-TTY) with no ANSI, and large output is spilled to a file.
- **Timeouts:** the `send` bound of 90 s is below Claude Code's default 120 s Bash timeout. For Codex, the skill says to set a command timeout of at least 100 s, or to use `--no-wait` + `glen wait --timeout`. Verify Codex's default in W4.

## §A14 Security and privacy
- Local only; no network.
- GLEN_HOME is 0700. The socket's only protection is filesystem permissions (a same-user boundary).
- The extension runs unsandboxed inside omp, like every omp extension. glen ships its source in the repo and installs it only on explicit `install-extension`.
- Message text is stored only under GLEN_HOME.
- glen never writes omp session files.

## §A15 Testing strategy
- Fixtures live under `tests/fixtures/` (see its README).
- The `GLEN_OMP_BIN` seam supports the fake omp now and the fake RPC peer in W2.
- An `Env` snapshot means code and tests never mutate the process environment.
- Integration tests use `CARGO_BIN_EXE_glen` with `env_clear()`.
- Conformance tests live in `tests/conformance.rs` (the `/implement` tail).

## §A16 Wave map
- **W0** foundation: this plan.
- **W1** disk reader: ls, show, tail and `-f`, handles, continuations, leaf rule, read-only lease probe.
- **W2** RPC driver: send/wait/jobs for sessions with no live owner, `@new`, the per-job worker, the per-session lock, the `glen_reply` host tool, fork detection, the fake RPC peer, the `omp-rpc` git dependency.
- **W3** live route: the bundled extension, install-extension, endpoint-first routing, inbox, talk-back from the TUI.
- **W4** agent surface: SKILL.md (two variants), install-skill, extended doctor, completions polish, acceptance runs with Claude Code and Codex.
- **W5** `glen watch` TUI.
- **W6** site (Astro; Nodex redesign + morphin motion).
- **W7** deploy + release workflow.
- **W8** audits + release v0.1.0.
- Every wave leaves `main` green and usable. Unfinished subcommands stay hidden.

## §A17 Verify on every omp upgrade (and open items)
- [ ] Re-read `packages/utils/src/dirs.ts` (§A12).
- [ ] Re-check session entry types, the title slot, `resolveSessionLeafId` and `findSessionContinuation`.
- [ ] Re-check both `CA/session/session-storage.ts` `tryAcquireSessionLease` and `packages/utils/src/file-lock.ts` `getLockPath`.
- [ ] Re-check the Linux abstract-socket name and datagram bind semantics.
- [ ] Re-check the RPC schema: `prompt_result`, `session_settled` and the `set_host_tools` duplicate rule.
- [ ] Re-check the extension API: `sendUserMessage` deliverAs, `registerTool` and session events.
- [ ] Bump `TESTED_OMP_VERSION` only after all of these checks.
- **Open, owned by later waves:**
  - W1: confirm the Linux probe semantics against `crates/pi-natives/src/file_lock/linux.rs`, and the `.lock` suffix against `utils/src/file-lock.ts` `getLockPath`. Add a test that the probe path for a fixture id ends in `<id>.lock`.
  - W2: a live pid alone does not prove the worker is alive (pids are reused). `lost` detection must check a worker identity beyond the pid, for example its process start time recorded in `status.json`.
  - W3: confirm the extension can read busy/idle state and observe persistence continuations.
  - W4: Codex's shell timeout and output cap; the Claude Code `allowed-tools` syntax.

