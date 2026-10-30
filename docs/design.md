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
| Headless RPC | `CA/cli/args.ts:392`, `CA/modes/rpc/rpc-mode.ts:1370-1373` | `--no-ui`: extensions have `hasUI=false` |
| Event filtering | `CA/modes/rpc/rpc-mode.ts:2444-2460` | `set_event_filter` gates session events only |
| Local prompt acknowledgement | `sdk/rust/omp-rpc/src/wire.rs` | `PromptAck.agentInvoked:false`: local completion, no result/settled wait |
| Persistence notice | `CA/modes/rpc/rpc-mode.ts:1350-1365` | `notice` with `source:"session-persistence"`, no ids; re-read `get_state` |
| RPC stdin EOF | `CA/modes/rpc/rpc-mode.ts:2937-2950` | EOF shuts omp down |
| Directories | `packages/utils/src/dirs.ts` | §A12 |

## §A3 The three channels
**Disk reader (W1)**
- **Discovery.** Buckets are the immediate subdirectories of `sessions_dir`. Session files are `<bucket>/*.jsonl` with a valid header. Skip dotfiles (`.lock.os`, `.*.tmp`), symlinked buckets and everything inside subdirectories. Follow symlinked regular files and deduplicate canonical paths. Header scanning falls back from 4 KiB to at most 64 KiB when no complete header fits.
- **Bucket for a cwd.** Port `getDefaultSessionDirName` (`CA/session/session-paths.ts`). Canonicalise the cwd. Canonicalise home and the temp root too (falling back to lexical absolute paths when unavailable). The temp root (`TMPDIR` → `TMP` → `TEMP` → `/tmp`) is checked first (`-tmp-<rel>`), then home (`-<rel>`), else `--<abs>--`. Separators `/ \ :` become `-`.
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
- **State column.** `idle|live|busy|unknown`, with `state_source:"disk"` in W1 (W3 adds `"extension"`) and nullable `state_detail`. A free lease means idle; an inconclusive probe means unknown. A held lease means busy when the last active-branch message is in flight: role user, developer or toolResult, or an assistant containing a toolCall whose stopReason is neither error nor aborted. Otherwise it means live. This predicate is separate from the last-turn state `complete|interrupted|aborted|error|pending|unknown`, ported from `session-listing.ts`.
  - If a held session's 32 KiB tail window contains no complete message, load its full active branch to determine state and last turn. Other rows retain bounded scans.
  - Probe only displayed rows, except when `--live` needs to filter candidates. A probe briefly holds a free lease; never repeat it inside the follow loop. Linux abstract sockets are network-namespace-local: a probe in another netns cannot observe the owner's socket.
- **Follow.** `tail -f` polls file metadata every 250 ms and complete appended lines only; a partial line waits for its newline. Check continuations every 2 s or immediately after disappearance. A missing file with no continuation for 5 s ends with `not_found`.
  - A size shrink or changed device/inode triggers a reload and `rewritten`, including omp's stage-and-rename replacement by a larger file. Emit only unseen active-branch entry IDs after a rewrite or continuation; report `branch_switch` when the previous leaf is no longer on the branch.
  - Polling avoids platform-specific FSEvents/inotify dependencies and survives replacement. Same-size title-slot edits intentionally do not trigger a reload. SIGINT ends the stream with `end` and exit 130; a second SIGINT exits 130 immediately.

**RPC driver (W2)**
- `omp-rpc` is pinned to git tag `v18.14.0` (not a path dependency).
- **Per-job detached worker.** `send` creates a durable job, then re-execs hidden `glen __worker <job-id>` in its own session (`setsid`), with stdin `/dev/null` and stdout/stderr to private `worker.log`. Neither it nor omp inherits the foreground pipes. Foreground observers never cancel the worker on Ctrl-C.
- **Worker lifecycle:**
  1. Claim the queued job with `worker_pid` and its process start token; register SIGTERM/SIGINT/SIGHUP cancellation.
  2. Hold glen's per-session flock through teardown. Poll every 200 ms while queued, checking abort and the absolute job deadline. Re-follow continuations after acquiring it; refuse a moved id with `session_moved` before starting omp.
  3. Probe the owner lease using the `owners_dir` frozen in `job.json`; held or unknown becomes `no_route`.
  4. Start `omp --mode rpc --no-ui [--profile NAME] [--resume <absolute file>]`, `GLEN_MANAGED=1`. An explicit profile removes `OMP_PROFILE` and `PI_PROFILE` from the child environment; otherwise both are inherited. Resume uses an existing header cwd, falling back to glen's cwd; `@new` uses `--cwd` or glen's cwd.
  5. On a startup thread, wait for ready, register exactly one `glen_reply` host tool, set the event allowlist, then take a `get_state` baseline. The main thread polls controls and the deadline every 200 ms. Cancellation sets a shared abandon flag and writes terminal status plus the control acknowledgement immediately, then joins startup. Startup checks abandon after every call, drops any late client, and never prompts an abandoned job. Worker exit waits for teardown and the join.
  6. Refuse a baseline id mismatch (`session_moved`); for `@new`, acquire the glen lock for the revealed id. Re-check cancellation/deadline and discard startup events. Do not re-probe our own child’s lease: startup recovery may already have claimed it.
  7. Submit a prompt on a scoped thread under the cancellation mutex, with a 100 ms ack wait. Read the SDK event receiver directly on the polling loop; drain it before finalization so pre-ack output cannot lag behind completion. A late ack still arrives as an unmatched response. The first session event or completed outcome writes `accepted`. The event filter excludes message/tool streaming updates while retaining terminal messages and notices.
  8. Complete on a completed `prompt_result` with `sessionSettled:true` or an observed post-prompt `session_settled`. An ack with `agentInvoked:false` completes locally without waiting for either; preserve `command_output`. A completed result frame with `agentInvoked:false` and an unsettled session waits at most 10 s, then completes with `settle_timeout`. Cancellation after a completed result preserves completion with `session_settled:false` and `settle_cancelled`. `final:true` replies never complete a job.
  9. Recheck identity, write `result.json` before terminal `status.json`, answer controls, close the client, then release the glen lock. `Client::close` terminates omp's private process group. Background commands the bash tool placed in separate groups can survive; glen does not hunt them.
- **Fork detection.** Re-read `get_state` on persistence notices, changed `session_info_update` ids and before finishing. A change from the baseline records `{from,to}` and returns exit 11 with both ids and `to_handle`; never infer ids from notice prose. Persistence errors add a warning.
- **Pre-spawn probe (AC-207, revised for W2 review cycle 1).** When the glen lock is free, `send` re-probes the owner lease after reading stdin; held or unknown refuses with exit 6 before creating state. The worker probes again immediately before spawning; held or unknown fails `no_route` without prompting. There is no pre-prompt probe: omp startup recovery writes can claim our own child’s lease before ready. Baseline identity checks and post-start fork detection remain mandatory. This supersedes the two-probe AC-207 in the original W2 plan.
- **Timeouts:**

| Bound | Value | Outcome |
|---|---|---|
| Foreground send/wait | default 90 s, 0 for no-wait | exit 10; worker continues |
| Null worker pid | 10 s spawn grace | reader persists lost |
| Absolute job deadline | `GLEN_JOB_TIMEOUT`, default 60m, from creation | queued/startup: failed timeout; running: abort; completed-unsettled: completed with `settle_timeout` |
| Owner lock wait | until job deadline, 200 ms polling | timeout or cancellation |
| Ready | min(60 s, time left) | startup unavailable; abandoned startup joins before exit |
| Setup calls | 10 s each | per-call error mapping |
| Running state calls | off the polling thread; at most min(10 s, time left) | final identity check awaited unless stopping; terminal status precedes a pending-call join |
| Abort calls and prompt ack | 100 ms each | late responses read from the event receiver; controls stay responsive |
| Prompt result | absolute job deadline | polling loop aborts when the deadline passes |
| Abort escalation | 10 s then SIGTERM; SIGKILL 1 s later | event receiver closes on EOF |
| Client teardown | SIGTERM, 1 s; SIGKILL, 1 s; bounded reader join | log teardown errors |

- Tests use the `fake_omp` example, never live omp (§A15).

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
3. A **held glen session lock** routes to RPC (queued) before consulting the owner lease: glen’s own child may hold that lease. Otherwise a free owner lease routes to RPC. The worker probes before spawning; held or unknown fails `no_route`. Startup may claim our own child’s lease, so it is not probed again before prompting.
4. **Lease held and no endpoint** → refuse: exit 6 `no_route`. W2 hint: "close that omp session and retry; sending into a running session needs the glen omp extension (a later glen version)". W3 replaces the hint with installation guidance.
5. **Probe result unknown** → refuse (`no_route`).

**Residual race, stated plainly.** omp claims the owner lease on its first write, so probe → spawn → prompt is not atomic. If a TUI opens the same session between glen's probe and the RPC child's first write, one of the two writers continues in a sibling file. glen narrows the window (endpoint first, a fresh routing probe, a pre-spawn probe, a per-session glen lock) and detects a fork afterwards (exit 11 `forked`, reporting both ids). It cannot prevent it. On macOS the probe itself holds the flock for microseconds; that is the same exposure `omp gc` has.
The same rule means an omp TUI that has **opened** a session but not yet written to it holds no lease: in W2, `ls` reports it `idle` and `send` goes through the RPC route (confirmed live against omp 18.14.0, W2 smoke step 5). Writes in that TUI afterwards continue from its in-memory leaf, so the two turns end up as sibling branches of one file rather than a forked file. The W3 glen extension closes this gap: it registers a live endpoint when the TUI starts, so `ls` shows `live` and `send` takes the live route before omp's first write.

## §A5 Reply channel (`glen_reply`)
- **Tool** `glen_reply`, params `{text: string, final?: boolean}`. Description: "Send a message back to the agent that contacted you through glen (Claude Code or Codex). Use it to answer a glen request or report progress." It returns `"delivered"`.
- **Exactly one provider per omp process.** omp rejects a host tool whose name an extension already registered.
  - glen's RPC children run with `GLEN_MANAGED=1`; the extension skips its tool and socket there, and the worker supplies a host tool via `set_host_tools`.
  - TUI and print sessions with the extension installed use the extension's tool.
- **Delivery:**
  - RPC: append in call order to `jobs/<id>/replies.ndjson`, then the inbox, and include the same records in `result.json.replies`. A job append error returns a tool error without an in-memory record; inbox errors are logged and the job reply is retained.
  - Live: the extension streams a `reply` event to a connected worker (when a glen request is active) and appends to `inbox/<sessionId>.ndjson`: `{v:1, id, session_id, job_id|null, request_id|null, tool_call_id|null, text, final, at}`.
- **Validation:** text must be a string, nonempty after trimming, at most 65,536 UTF-8 bytes; optional `final` must be boolean. Invalid calls record nothing and return a tool error. `final:true` is data only, not completion. W2 writes the private inbox; reading it ships in W3.
- **Reading:** `glen inbox` shows unread messages, tracked by a per-session byte-offset cursor in `inbox/.read/<sessionId>`, and marks them read unless `--peek`. `--follow` streams NDJSON.

## §A6 glen state (`GLEN_HOME`)
```
$GLEN_HOME            default ~/.glen; must be absolute; leading ~ expanded; relative → invalid_env (exit 2)
├── run/live/<sessionId>.json, run/live/<pid>.sock    W3 live endpoints
├── run/locks/<key>.lock                        W2 per-session request lock
├── jobs/<jobId>/{job.json,status.json,result.json,worker.log}   W2
├── jobs/<jobId>/replies.ndjson, control/<name>.{json,done.json}   W2
├── inbox/<sessionId>.ndjson                                    W2 writes
├── inbox/.read/<sessionId>                                     W3 reads
└── out/<unix-ms>-<command>-<pid>.{json,txt}                      W0 spilled output
```
- **Lock keys:** a plain token session id is its own key; other ids use `h-<xxh64(id,0)>`. These are glen locks, never omp append gates. Lock files are never unlinked.
- **Modes:** directories glen creates are 0700, files 0600. glen never chmods a GLEN_HOME that already exists; doctor warns about group/other bits.
- **Atomic writes:** a temp file in the same directory, then rename.
- **Socket paths:** `sun_path` holds at most 104 bytes on macOS. A socket path that is too long is refused, with a hint to shorten GLEN_HOME.
- **Retention:**
  - `out/` files older than 7 days are pruned on every spill.
  - Only terminal jobs (including lost) whose parsed status `updated_at` is more than 7 days old are pruned when a new job is created. Active or unreadable jobs are kept; pruning is best effort.
  - Inbox files are kept.

## §A7 Jobs
- **Id:** glen uuidv7; **handle:** `j` + last 7 hex. `wait` accepts a full uuid or `j` plus at least 7 hex suffix digits, case-insensitively. Multiple matches return `ambiguous` with candidates; bad shapes return usage with a `glen jobs` hint.
- **`job.json`:** `{v,id,handle,created_at,route,session_id?,session_file?,session_handle?,deliver_as,text,cwd,omp_bin,profile_flag?,owners_dir,job_timeout_ms,deadline_at}`. W2 routes are `rpc|new` (W3 adds live). `owners_dir` and `omp_bin` are frozen absolute paths resolved by send. Optional disk fields are omitted when absent; readers tolerate unknown fields.
- **`status.json`:** `{v,status,updated_at,worker_pid?,worker_start?,omp_pid?,request_id?,actual_session_id?,actual_session_file?,delivered_as?,waited_ms?,waiting_for_lock?,forked?:{from,to},error?:{code,message,hint?,details?},warnings:[]}`.
- **`result.json`:** `{v,prompt_status?,agent_invoked,assistant_text?,replies:[],command_output:[],stop_reason?,session_settled,settled_at?}`. Result is published before terminal status.
- **Queue display:** `waiting_for_lock:true` is published only after observing the glen lock held, and cleared on acquisition before omp startup. `waited_ms` retains the acquired lock’s wait duration. Human observers show "queued behind another glen job" only while that marker is true, "starting omp" otherwise, and "waiting for the reply" after acceptance.
- **Statuses:** `queued → accepted → completed|aborted|failed`, and `queued|accepted → lost`. A worker checks terminal state before claiming.
- **Lost identity:** readers compare the pid and process start time (Linux `/proc/<pid>/stat` starttime; macOS `proc_pidinfo` start timestamp). A gone process, zombie or mismatched token persists `lost`. Unknown identity is not evidence of loss. With a null pid only, status older than 10 s exceeds spawn grace. A live matching worker is never marked lost. Loss detection re-reads status under the same status lock used by worker publications, preserving any newer terminal outcome.
- **Unsupported platforms.** Only Linux and macOS provide process start tokens. On other Unix, or when a token cannot be read, a worker records its pid without `worker_start`; readers conservatively cannot mark that job lost, even if it later dies. The null-pid grace does not apply to a recorded pid. This limitation is unsupported-platform behavior, not a fallback to the null-pid grace.
- **Deadline:** `GLEN_JOB_TIMEOUT` is parsed at job creation (1ms–24h, default 60m); invalid/zero values warn and use 60m. The recorded absolute deadline never resets. Before a result, timeout aborts and fails with `timeout`; after a completed result but before settlement it completes with `session_settled:false` and warning `settle_timeout`.
- **Job error codes:** `no_route`, `timeout`, `cancelled`, `aborted`, `omp_error`, `omp_exited`, `omp_unavailable`, `reply_tool_rejected`, `session_moved`, `rpc_error`, `protocol`, `io`, `spawn`, `lost`, `internal`.
- **Exit mapping:** completed → 0 (11 when forked); failed `no_route` → 6; other failed/aborted/lost → 7 `job_failed` with `{status,job,error,forked}` details. A bound expiring while queued/accepted → 10 with `ok:true` and a running document. Foreground SIGINT prints that document with `interrupted:true` and exits 130; worker/omp continue. Worker SIGTERM/SIGINT/SIGHUP abort with `cancelled`.
- **Control seam:** atomically drop `jobs/<id>/control/<name>.json` with `{"v":1,"op":"abort"}`. The worker polls every 200 ms, including while queued or starting, and acknowledges `<name>.done.json` with `{"v":1,"ok":true}`, then removes the request. Unsupported/malformed operations return `{"v":1,"ok":false,"error":{"code":"unsupported_op","message":"…"}}` and continue the job. Dotfiles and `.done.json` are ignored. Startup cancellation publishes terminal status/ack before joining the bounded startup thread; no prompt follows abandonment.
- **Reserved W3b operations (not implemented):** `set_model {provider,model_id}`, `set_thinking_level {level}`, `steer {text}`, `follow_up {text}`. Dispatch them on the same held client; never start a separate omp process for controls.

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
  3. Otherwise match case-insensitively over all sessions in `sessions_dir`. Take the union of: the full id; a hex suffix of the dash-free id (at least 4 hex); an omp-style prefix of the id or file name (at least 8 chars); a session whose catalog-assigned handle (as `ls` prints it, including a 7-char legacy prefix) equals the argument. Every handle `ls` prints therefore resolves back to its session.
     - One match → resolved.
     - Several → exit 4 `ambiguous`, with the candidates in `error.details.candidates`.
     - Two files whose headers carry the same session id (a copied file) are two candidates even for a full-id match. glen never picks one silently, because omp keys the lease by id and both copies share it.
     - None → exit 3 `not_found`.
  4. Then follow continuations.
- `@new` is valid only for `glen send`.

## §A9 Commands (v0.1.0)
| Command | Purpose | Wave | Visible |
|---|---|---|---|
| `glen ls [--all] [--cwd DIR] [-n N] [--live] [--since DUR]` | List sessions for the cwd bucket (`--all`: every bucket); `--all` conflicts with `--cwd`; `-n 0` lists all. Columns: handle, state, title, last turn, updated ("3m ago"), model; plus project with `--all` | W1 | W1 |
| `glen show <session> [--thinking] [--tools collapsed\|full\|none] [--raw]` | Active-branch transcript; `--raw` = entries as JSON | W1 | W1 |
| `glen tail <session> [-n N] [-f] [--thinking]` | Last N transcript items; `-f` follows (NDJSON in JSON mode); `-n 0` gives no backlog | W1 | W1 |
| `glen send <session\|@latest\|@new> <message\|-> [--as auto\|steer\|follow-up] [--steer\|--follow-up] [--wait DUR\|--no-wait] [--cwd DIR]` | Send through the live route or RPC, then wait up to the bound (default 90s) | W2 (RPC), W3 (live) | W2 |
| `glen wait <job> [--timeout DUR]` | Keep waiting for a job (default 90s) | W2 | W2 |
| `glen jobs [--session S] [--active] [-n N]` | List glen's jobs | W2 | W2 |
| `glen inbox [--session S] [--peek] [--all] [-f]` | Messages omp sent back with `glen_reply` | W3 | hidden |
| `glen watch [<session>]` | Full-screen live view (ratatui) | W5 | hidden |
| `glen install-extension [--dry-run] [--force] [--uninstall]` | Install the bundled omp extension for the effective agent dir/profile | W3 | hidden |
| `glen install-skill [--claude] [--codex] [--project] [--dry-run] [--force] [--uninstall]` | Write SKILL.md for Claude Code and/or Codex | W4 | hidden |
| `glen doctor` | omp binary and version, resolved paths, glen home (later: extension, skill, endpoints) | W0 (extended W3/W4) | visible |
| `glen completions <bash\|zsh\|fish\|elvish\|powershell>` | Shell completion script (raw) | W0 | visible |
- **Send rules:** `--steer` / `--follow-up` alias `--as` and conflict with explicit `--as` or each other. `--cwd` applies only to `@new` (new session cwd, must exist) and `@latest` (project selection). Message `-` reads stdin; empty, non-UTF-8 or more than 512 KiB is usage. RPC auto/follow-up sends `streamingBehavior:followUp`, steer sends `steer`; idle baseline reports `delivered_as:prompt`.
- **Global flags:** `--json`, `--human` (mutually exclusive), `--color auto|always|never`, `--profile NAME`, `-h`, `-V`.
- **Defaults and aliases:** `--color` defaults to `auto`; `--profile` overrides `OMP_PROFILE` and `PI_PROFILE`. `ls` and `jobs` use `-n/--limit` (default 20); `tail` uses `-n/--lines` (default 10). `tail` and `inbox` accept `-f/--follow`. `show --tools` defaults to `collapsed`; `send --as` defaults to `auto`; `send --wait` and `wait --timeout` default to `90s`. `-h/--help` and `-V/--version` are available on every command. Unfinished commands remain hidden and return `not_implemented` (exit 8).
- **Durations:** `<n>ms|s|m|h` or bare seconds, at most 24h. `--wait 0` equals `--no-wait`. `ls --since` accepts `<n>s|m|h|d|w` or bare seconds, at most 3650 days.
- **Internal:** `glen __worker <job-id>` (W2), always hidden.
- **Considered and rejected:**
  - a `glen reply` CLI: the reply path is the `glen_reply` tool, and an omp Bash call has no job context;
  - a separate `glen steer` verb: folded into `send --as`;
  - a `--new` flag: an optional first positional is ambiguous, so it became `@new`;
  - an MCP server: not needed for v0.1.0.

## §A10 Output contract
- **Modes.** `--json` → JSON; `--human` → human; otherwise human when stdout is a TTY and JSON when it is not. Raw commands (`completions`) print raw unless `--json` is given. `--help` and `--version` are always text. Without `--json`, a raw command renders every error — usage, env (e.g. a relative `GLEN_HOME`, AC-020) and internal alike — as human text on stderr with an empty stdout, so stdout is never anything but the script itself (e.g. `glen completions zsh >> ~/.zshrc` can never append an envelope); with `--json` the error is the usual JSON envelope on stdout.
- **Streams.** stdout carries only the result. stderr carries diagnostics, warnings (human mode) and spinners. In JSON mode stderr stays empty except for panics.
- **Envelope** (`v` = contract version 1; fields are append-only; keys are snake_case; timestamps are RFC 3339 UTC; durations are `*_ms` integers; JSON paths are absolute):

| Document | JSON contract |
|---|---|
| Success | `{"v":1,"ok":true,"data":{…},"warnings":[{"code","message"}]?}` |
| Error | `{"v":1,"ok":false,"error":{"code","message","hint"?,"details"?},"warnings"?}` |
| Spilled | `{"v":1,"ok":true,"spilled":{"path","bytes","format":"json"|"text"}}` |

  - Pretty-printed on a TTY, compact otherwise.
  - `ok:true` holds for exits 0, 10, 11 and observer interruption 130.
- **W0 doctor data:** `data.glen` contains `version`, `home` and `home_exists`; `data.omp` contains `bin` (path or null), `version` (string or null) and `tested_version` (`18.14.0`). `data.paths` contains the §A12 resolved paths, `profile` (name or null), `agent_dir_source` (`default|profile|env`) and `sessions_dir_source` (`agent_dir|xdg`). `data.checks` is an ordered array of `{name,status,value,detail}`, with status `ok|warn|fail|info`. `value` is the raw absolute path or check value; `detail` is the reason for the status, or an empty string when there is none. The checks are `omp_binary`, `omp_version`, `profile`, `agent_dir`, `sessions_dir`, `glen_home`. An untested version succeeds with warning `omp_version_untested`. A failed doctor returns `omp_unavailable`, with the same data under `error.details.report`, and its human table before the error line. Missing directories are reported without creating them.
- **W1 reader documents.** `ls` returns `{scope:{all,cwd,bucket},sessions_dir,total,total_all,sessions:[]}`. Scope cwd/bucket are null with `--all`; total is after filtering but before limiting. Each session has `handle,id,path,bucket,cwd,title,first_message,created_at,updated_at,age_ms,size_bytes,model,last_turn,state,state_source,state_detail,continuation_of,parent_session`; nullable fields remain present as null. Future mtimes saturate age_ms at zero (human: `just now`).
  - `show` and document `tail` return `{session,continued_from,leaf_id,skipped_lines,items:[]}`. `show --raw` replaces items with active-branch `entries:[]`, including unknown entry kinds. `tail -n 0` returns an empty items array.
- **W2 send/wait documents:** completed data is `{status:"completed",job,session:{id,handle,path},assistant_text,replies,command_output,stop_reason,session_settled,delivered_as,forked}`. `forked` is null or `{from,to,to_handle}`. RPC session identifies the original resolved target; new route uses actual session identity. Nullable output fields remain present.
  - Running/interrupted data is `{status:"running",job,session,next:"glen wait <handle>",interrupted}`; workers keep running after exits 10/130.
  - Job view: `{id,handle,status,route,deliver_as,delivered_as,session_id,session_handle,actual_session_id,created_at,updated_at,elapsed_ms}`.
  - `jobs` data: `{total,jobs:[]}`, newest first; total is after filters/before limit. Rows add `text_preview` (first line, controls dropped, up to 80 characters), `error` and `forked`. `-n 0` means all; active filtering runs after lost detection. `--session` matches original or actual id, or at least 4 hex suffix digits. Missing GLEN_HOME yields an empty list without creating it.
  - Human job states: `○ queued`, `◐ running`, `✓ completed`, `✓ forked → <handle>`, `! aborted`, `✗ failed`, `✗ lost`; completed output includes assistant text and `↩` replies (`✓` on final).
- **Transcript items.** Every item has `kind`, `id` and nullable UTC `timestamp`. Thinking is hidden by default; tool results are folded into matching tool calls when present in the same build.

| Kind | Fields beyond the common fields |
|---|---|
| `user` | text, images |
| `assistant` | text, model, stop_reason, error, has_thinking, thinking_words; thinking only with `--thinking`; nonempty tool_calls unless `--tools none` |
| `tool_result` | tool_call_id, tool_name, is_error, text, chars, truncated, images |
| `bash`, `python` | command/code, exit_code, cancelled, output, chars, truncated |
| `developer` | text |
| `custom` | custom_type, text |
| `file_mention` | files: [{path,line_count}] |
| `compaction` | tokens_before, tokens_after, summary |
| `reset` | no additional fields |
| `branch_summary` | summary |
| `model_change` | model |
| `thinking_level` | level |

  - ToolCall carries `id,name,summary,result` and arguments only with `--tools full`. A result carries `is_error,text,chars,truncated,images`; collapsed text/output is limited to 500 characters, full retains it all. Tool summaries and first-message previews retain at most 120 and 200 characters respectively. `--tools none` omits tool calls/results and bash/python items. Unknown entry kinds stay opaque in the tree; unknown visible content blocks render `[type]`.
  - W1 follow events are `session` (session, continued_from, skipped_lines), `item` (item), `continued` (from, to, path), `rewritten` (path), `branch_switch` (leaf_id), `warning` (code, message), and final `end`, or an error-envelope line. Streams flush each line and never spill. SIGINT writes `end` and exits 130; a second SIGINT exits immediately. A tool result whose call was already streamed appears as a standalone tool_result.
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
- **Glyphs:** ✓ ok, ! warn, ✗ fail, ● live, ◐ busy, ○ idle, ? unknown, · info.
- **Paths:** `~/…` in human output, absolute in JSON.
- **Untrusted text.** Every string glen did not author (paths, env values, session titles, cwds, previews, model output) passes through `render::sanitize` before it is painted in human mode. Escape controls; never silently strip them. Human single-line fields such as titles, previews and tool summaries first collapse whitespace runs (including newlines and tabs) to one space, then sanitize. A literal ESC followed by `[31m` is displayed literally as `\x1b[31m`. JSON fields retain the raw value, including controls and whitespace, with serde escaping on the wire; documented preview/result length limits still apply.
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
| 6 | `no_route` (foreground or worker-side refusal) | live owner without a glen endpoint, or probe unknown; refusing to fork |
| 7 | `job_failed` | omp reported aborted/error, or the worker was lost (`details.status`) |
| 8 | `not_implemented` | subcommand declared but not in this build |
| 10 | (ok) `running` | wait bound elapsed; job continues; use `glen wait <job>` |
| 11 | (ok) `forked` | omp moved the session to a continuation; use the new handle |
| 130 | — | interrupted (SIGINT); send/wait print a running document with `interrupted:true`; a detached job keeps running |

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
- `GLEN_OMP_BIN` points to the `fake_omp` cargo example (`tests/support/fake_omp.rs`). It supports version, RPC and `hold-lease` modes. `cargo test` builds examples; `tests/common::fake_omp_bin` builds it on demand for a filtered run. It is not installed by cargo install.
- `FAKE_OMP_SCENARIO`: ok, local, progress, bad_reply, unsettled, never_settles, error, aborted, crash, hang, wedge, fork, fork_silent, moved, reject_tools, reject_filter, no_ready, delayed_state, fork_reply, local_result_unsettled. Timing/shape knobs: `FAKE_OMP_DELAY_MS`, `FAKE_OMP_READY_DELAY_MS`, `FAKE_OMP_SETTLE_DELAY_MS`, `FAKE_OMP_REPLY_BYTES`, `FAKE_OMP_OWNERS_DIR`, `FAKE_OMP_V2=1|partial`. `FAKE_OMP_LOG` records start, inbound/outbound frames and exits as NDJSON. `FAKE_OMP_STARTUP_RECOVERY=1` makes the fake claim its lease and append a recovery record to the copied fixture before ready. `FAKE_OMP_STATE_DELAY_MS` delays running state replies without blocking abort. `FAKE_OMP_RUN_RELEASE` holds prompt work until a test-owned file exists. `FAKE_OMP_READY_RELEASE` holds ready until a test-owned file exists (20 s bound). `FAKE_OMP_PROMPT_SUBMIT_DELAY_MS` delays the worker’s submission thread to exercise the cancellation gate.
- Process tests wait for terminal status, worker exit and the omp group to disappear before dropping scratch directories. The startup-abort regression delays ready by 5 s and checks terminal status/ack within 1.5 s with no prompt.
- Live Step 15 smoke is separate orchestrator evidence: use fresh scratch PI_CODING_AGENT_DIR and cwd, never resume a user session. It remains unverified in the sandbox.
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
- [ ] Re-check session entry types, the title slot, `resolveSessionLeafId`, `findSessionContinuation` and `deriveSessionStatus`.
- [ ] Re-check both `CA/session/session-storage.ts` `tryAcquireSessionLease` and `packages/utils/src/file-lock.ts` `getLockPath`.
- [ ] Re-check the Linux abstract-socket name, datagram bind semantics and XXH64 golden vectors.
- [ ] Verify `--no-ui`, event-filter scope, settlement after results for started runs, and `agentInvoked:false` local semantics.
- [ ] Live scratch smoke: verify `omp --mode rpc --resume` does not claim its owner lease before the first prompt; retain Step 15 evidence (not yet run in the sandbox).
- [ ] Re-check the RPC schema: `prompt_result`, `session_settled` and the `set_host_tools` duplicate rule.
- [ ] Re-check the extension API: `sendUserMessage` deliverAs, `registerTool` and session events.
- [ ] Bump `TESTED_OMP_VERSION` only after all of these checks.
- **Open, owned by later waves:**
  - [x] W1: confirmed Linux abstract datagram bind semantics against `crates/pi-natives/src/file_lock/linux.rs:14-22` and the `.lock` suffix against `utils/src/file-lock.ts:31-33`. Tests cover both golden names and `<id>.lock` path suffixes.
  - [x] W2: lost detection records pid plus process start time, rejects zombies/reused pids, and gives null-pid starts a 10 s grace.
  - W3: `glen_reply` from the extension must stay disabled under `GLEN_MANAGED=1`. Confirm the extension can read busy/idle state and observe persistence continuations; replace disk-inferred busy with exact extension state.
  - W4: Codex's shell timeout and output cap; the Claude Code `allowed-tools` syntax.
