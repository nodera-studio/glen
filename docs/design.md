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
| Delivery and queue | `CA/extensibility/extensions/types.ts:1159-1172`, `CA/session/agent-session.ts:9097-9101` | before_agent_start signals prompt delivery; batches join with two newlines; hasPendingMessages reads the queue |
| Extension settlement | `CA/session/agent-session.ts:1835-1866,5703-5735,9026-9040` | Extension agent_end keeps original flags; session tail re-tags; prompt bracket unwinds later |
| Extension tools and host | `types.ts:971-1020`, `docs/extensions.md:62,379,437-458` | essential/read; factories rebind per subagent; ctx.agent gates main; managed timers isolate errors |
| Explicit extension | `CA/cli/args.ts:382` | `--no-extensions -e FILE` still loads the explicit file |
| Directories | `packages/utils/src/dirs.ts` | §A12 |

**Session controls (W3b).** `rpc-mode.ts` implements `abort`, `get_available_models`, `set_model {provider,modelId}`, `get_available_thinking_levels`, `set_thinking_level {level}`, and `compact {customInstructions?}`. Ordinary commands are serialized: abort does not overtake a registry refresh or compaction. Only prompt/steer/follow_up and a few side channels bypass that queue.

`ModelControls.setModel` appends a default-role `model_change` even when unchanged, requires auth, re-applies thinking, and records model usage. `setThinkingLevel` clamps to model capabilities and appends `thinking_level_change {thinkingLevel,configured}` only when the effective level changes. RPC and the extension do not pass persist; global settings stay unchanged. Resume restores the last default-role (or absent-role) model change on the active branch; assistant provider/model is only a fallback before an explicit default, excluding `<synthetic>`. The last thinking change carries configured; no thinking entry means the level comes from settings/model defaults, so disk reporting is null.

Manual compaction aborts and later resumes a running turn. Explicit abort cancels manual compaction but keeps queued steer/follow-up messages queued without starting them. `ctx.isIdle()` only reflects streaming, so glen separately tracks compactions it starts. Session files are lazy until the first assistant answer; earlier model/thinking changes are in memory. `--no-session` creates an in-memory session and cannot combine with resume.

The extension uses `ctx.abort/isIdle/hasPendingMessages/compact/getContextUsage/model/models.list`, and `pi.setModel/getThinkingLevel/setThinkingLevel`. `ctx.models.list()` uses available models without RPC's enabled-model filter; supported levels are off plus thinking.efforts only for reasoning models.

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
  8. Complete on a completed `prompt_result` with `sessionSettled:true` or an observed post-prompt `session_settled`. An ack with `agentInvoked:false` completes locally without waiting for either; preserve `command_output`. A completed result frame with `agentInvoked:false` and an unsettled session waits at most 10 s, then completes with `settle_timeout`. Cancellation after a completed result preserves completion with `session_settled:false` and `settle_cancelled`. EOF after an unsettled completed result ends the wait immediately with `settle_timeout`; reconcile any outstanding prompt acknowledgement before treating EOF as failure. `final:true` replies never complete a job.
  9. On every completed outcome, recheck identity with a separate 1 s budget (including completed-unsettled deadline and cancellation paths); skip this check for aborted or failed outcomes. Write `result.json` before terminal `status.json`, answer controls, close the client, then release the glen lock. `Client::close` terminates omp's private process group. Background commands the bash tool placed in separate groups can survive; glen does not hunt them.
- **Fork detection.** Re-read `get_state` on persistence notices, changed `session_info_update` ids and before finishing. A change from the baseline records `{from,to}` and returns exit 11 with both ids and `to_handle`; never infer ids from notice prose. Tag running queries with increasing sequence numbers and ignore older results after a newer identity was applied. Unmatched responses from timed-out state calls cannot publish identity. Before recording a `glen_reply`, resolve identity through a fresh bounded state query on the worker, so a reply arriving during a fork query cannot enter the old inbox. Persistence errors add a warning.
- **Pre-spawn probe (AC-207, revised for W2 review cycle 1).** When the glen lock is free, `send` re-probes the owner lease after reading stdin; held or unknown refuses with exit 6 before creating state, unless a fresh glen-lock check now finds a worker to queue behind. The worker probes again immediately before spawning; held or unknown fails `no_route` without prompting. There is no pre-prompt probe: omp startup recovery writes can claim our own child’s lease before ready. Baseline identity checks and post-start fork detection remain mandatory. This supersedes the two-probe AC-207 in the original W2 plan.
- **Timeouts:**

| Bound | Value | Outcome |
|---|---|---|
| Foreground send/wait | default 90 s, 0 for no-wait | exit 10; worker continues |
| Null worker pid | 10 s spawn grace | reader persists lost |
| Absolute job deadline | `GLEN_JOB_TIMEOUT`, default 60m, from creation | queued/startup: failed timeout; running: abort; completed-unsettled: completed with `settle_timeout` |
| Owner lock wait | until job deadline, 200 ms polling | timeout or cancellation |
| Ready | min(60 s, time left) | startup unavailable; abandoned startup joins before exit |
| Setup calls | 10 s each | per-call error mapping |
| Running state calls | off the polling thread; at most min(10 s, time left) | terminal status precedes a pending-call join; every completed outcome awaits a separate 1 s final identity check |
| Abort calls and prompt ack | 100 ms each | late responses read from the event receiver; controls stay responsive |
| Prompt result | absolute job deadline | polling loop aborts when the deadline passes |
| Abort escalation | 10 s then SIGTERM; SIGKILL 1 s later | event receiver closes on EOF |
| Client teardown | SIGTERM, 1 s; SIGKILL, 1 s; bounded reader join | log teardown errors |

- Tests use the `fake_omp` example, never live omp (§A15).

**Live route (W3)**
- The single-file, erasable TypeScript extension (`extension/glen/index.ts`) uses only `node:` runtime imports. omp runs it with Bun; Node ≥22.6 runs its tests. `install-extension` embeds and renders it with an absolute GLEN_HOME and version. Runtime `GLEN_HOME` overrides the baked home; `~` expands. A process-wide `Symbol.for("glen.extension")` guard suppresses a second installed/`-e` copy, while the same module can rebind for subagents.
- `GLEN_MANAGED=1` registers **nothing** (no tool, handlers, endpoint or files). Otherwise the factory registers `glen_reply` with `loadMode:"essential"`, `approval:"read"` in every mode. Only a main-agent `session_start` in mode `tui` binds an endpoint; subagents and RPC/ACP/print hosts never bind one.
- Bind lifecycle: require an absolute home and a plain-token session id (`^[A-Za-z0-9_-]{1,128}$`); create `run/live` 0700, sweep dead-pid registries/sockets, unlink the process's leftover socket, bind `<pid>.sock`, chmod 0600, then atomically publish `<lock_key(sessionId)>.json` 0600 **before the handler resolves**. Registry fields: `{v:1,pid,session_id,session_file:null|string,socket,ext_version,omp_version:null|string,protocol:1,mode,started_at,cwd:null|string}`. Optional title is understood by Rust. Version import has a managed 500ms cap. No omp transcript or append gate is ever opened for writing.
- The resolver unions endpoint-only sessions with the disk catalog, assigning handles across both. They are addressable by id, handle and `@latest`, and appear in `ls` with `state_source:"extension"`. `show`/`tail` return an empty transcript and `not_yet_persisted`; `tail -f` waits for persistence. This is added **AC-326** (orchestrator O3). A missing cwd uses the invoking project for catalog scope.

| Protocol v1 item | Contract |
|---|---|
| Transport | UTF-8 NDJSON, one request per connection; extra request lines ignored |
| Request | `{v:1,id:string,op,args:{…}}`; hello/state may omit args |
| Response | Exactly one `{v:1,id,ok:true,data:{…}}` or `{v:1,id,ok:false,error:{code,message,details?}}` |
| hello | Args `{client,client_version,protocols:[1]}`; data `{protocol:1,protocols:[1],ops:["hello","state","send","abort","list_models","set_model","set_thinking_level","compact"],ext_version,omp_version,pid,session_id,session_file,mode,state:"idle"|"busy",busy,queued,active_requests}` |
| state | Same data without protocol/protocols/ops; identity is re-read on every request |
| send | Args `{text,deliver_as:"auto"|"steer"|"follow_up",job_id:null|string,expect_session_id:null|string}`; ack data `{delivered_as,session_id,request_id}` precedes every event |
| Events after successful send | `consumed` at most once; `reply {reply}`; `session_changed {from,to,reason}`; exactly one terminal `settled {status,assistant_text,stop_reason,error_message}` or `error {error:{code,message}}`, then close |
| Errors | `unsupported_version` details `{supported:[1]}`; `unsupported_op` details `{op,ops,reserved}`; `bad_request`, `session_mismatch` details `{session_id}`, `send_failed`, `internal` |
| abort | Args `{expect_session_id?}`; data `{session_id,was:"busy"|"idle",aborted,idle_after,queued}`. Idle does not call ctx.abort. Busy calls it once and waits at most 3s in managed 50ms ticks. |
| list_models | Args `{expect_session_id?}`; data `{session_id,current:{provider,id}|null,thinking_level,models:[{provider,id,name,reasoning,thinking_levels,context_window,max_tokens}]}` |
| set_model | Args `{provider,model_id,expect_session_id?}`; data `{session_id,model:{provider,id,name},previous:{provider,id}|null,thinking_level,thinking_levels,busy}` |
| set_thinking_level | Args `{level,expect_session_id?}`; data `{session_id,thinking_level,previous,model:{provider,id}|null,available,busy}`; reject unsupported levels before the setter |
| compact | Args `{instructions?,expect_session_id?}`; data `{session_id,tokens_before,tokens_after}`. Refuse streaming, queued messages, or another glen compaction. Instructions ≤524288 UTF-8 bytes. |
| Capabilities | hello.ops is authoritative; no reserved ops. Unknown ops return unsupported_op with `{op,ops,reserved:false}`. Missing capability or unsupported_op becomes no_route/extension_outdated and install/restart guidance. |
| Control errors | model_not_found, no_auth, invalid_level (details available/model), busy, omp_error, timeout, job_ended, plus existing protocol errors. Identity is checked before effect and re-read after each awaited call. |
| Limits | Request line ≤4 MiB; client frame ≤16 MiB; text ≤524288 UTF-8 bytes; no complete request within 5s closes the connection |

- Unknown fields and unknown client event kinds are ignored. Malformed JSON, invalid UTF-8, missing string id or oversized request returns `bad_request` (unknown id null). Duplicate held/settled request ids are rejected before injection (bounded settled-id history). Session mismatch never injects.
- **Delivery:** idle means `pi.sendUserMessage(text)` with no options, for every requested delivery mode (ack `prompt`). Busy steer uses `{deliverAs:"steer"}`; busy auto/follow_up uses `{deliverAs:"followUp"}`. Explicit delivery queues even when idle, so glen never supplies it while idle. `sendUserMessage` returns void.
- **Consumption and attribution (O6 amendment):** push each request before injection and buffer synchronous events until the acknowledgement. Only `message_end` with role `user` and text equal to one pending injection consumes it, in submission order; record the event sequence as its consumption identity. `agent_start` and joined `before_agent_start.prompt` never prove delivery. One boundary can consume at most one request. Replies and assistant messages belong only to the currently open request span; a user message always clears prior ownership (except adjacent queued requests in the same batch), while custom/developer/system/async-result notices clear it only after the span contains assistant text; before that, the waiting span retains ownership. Disconnecting retains the span and durable inbox delivery.
- **Settlement:** close the current span as soon as the next user message starts, regardless of whether it has an answer, except for adjacent queued glen requests sharing the same batch (`message_end` is the fallback for hosts that omit starts). A custom/async-result/system-notice message (including other non-assistant/non-toolResult notice roles) closes the span only if it already contains assistant text; otherwise the span continues past the notice, retaining ownership until its answer arrives. Assistant starts, tool-only assistant messages and `glen_reply` records do not count as assistant text for this rule. A last span closes when the terminal main-agent `agent_end`'s managed 50ms poll observes idle and no pending messages, bounded at 30s and invalidated by a later agent start or pending messages; a later terminal end re-arms it. Never settle an unconsumed request as completed: an undelivered submission has a 30s delivery deadline, and an unconsumed queued message at quiescence fails `not_delivered`. Delivery timers are cancelled on consumption or termination.
- Assistant text is the last assistant text inside the span. Assistant text/stopReason/errorMessage come only from assistant `message_end` inside that request's span, never from `agent_end.messages` or a later request's assistant output. Text blocks join with newlines; an empty/tool-only span gives null text. `aborted`/`error` stopReason maps to those states; everything else completed. The Rust live driver uses the per-request `settled` event unchanged as the job's result source.
- **Identity:** every main event and socket request compares the current session manager id/file. Publish the new registry before pid-checked removal of the old one. Emit `session_changed`, reason `new|resume|fork` from session_switch, otherwise `moved`. The `session_switch` event terminates old requests on new/resume with `session_switched`, even if hello/state already refreshed the cached id in the event gap; requests submitted for the new id in that gap survive; moved/fork remain observed and report exit 11. A non-token id unregisters and terminates requests.
- **Cleanup:** shutdown emits `error shutdown`, closes the server and unlinks its socket and pid-owned registry synchronously within omp's 2s budget. A synchronous process exit backstop repeats the unlinks. Raw callbacks are guarded, socket/server errors have listeners, timers use the managed ctx API (socket idle timeout is the Node socket API). No terminal output; TUI notices use `ctx.ui.notify`.
- `GLEN_EXT_TRACE=<absolute file>` appends guarded NDJSON event/flags/idle/queue/request state snapshots. Step 14 was **run on the host — see evidence** in `.claude/evidence/v010-wave-3-live/README.md`, `05-trace.ndjson` and `05-transcript.txt`. Step 5 failed attribution under the original idle-only rule; the O6 amendment above supersedes it. Re-run Step 5 on the host after this fix; fake-host replay is regression evidence, not live acceptance.

**Watch (W5)**
- Four long-lived readers own their I/O and caches and send messages to the UI; the reducer and view do no I/O. Sessions refresh every 2s, or immediately on refresh/scope changes. Follow polls every 250ms. Jobs refresh active statuses every 250ms and list the store every 1s. Inbox reads new records every 500ms and re-reads persisted cursors every 1s.
- **Probe on change.** A disk row with a previously free lease skips the next probe while its device, inode, size and mtime are unchanged. Changed files, held leases and unknown leases are probed again; matching extension-backed rows are never probed. Head and refined-tail caches avoid rescanning unchanged files.
- **Shared cores.** Watch reuses `tail -f`'s `Follower` and `send`'s `submit` path; the extraction preserves CLI output byte-for-byte and retains the same routing/refusal behavior. The transcript updates per persisted entry, not per token; partial lines wait for their newline.
- **Peek-only inbox.** Watch never marks replies read or writes `inbox/.read`; persisted cursors only inform unread counts.
- **Controls gate.** W3b has not landed in this base: `controls::AVAILABLE = false`, and the controls adapter is a stub. Unavailable controls show “session controls are not in this build”.

## §A4 Routing (`glen send`)
1. Resolve the target including endpoint-only sessions, follow continuations, read/validate text. Target resolution for send does **not** probe the lease. `@new` always uses RPC.
2. Before consulting either lock, inspect `run/live/<lock_key(id)>.json`. A 2s matching hello with protocol 1 and send in ops selects live; no omp binary or owner-lease probe is needed. If the glen session lock is also held, refuse exit 6 with `glen jobs --active --session <handle>` guidance. Extension version mismatch warns with both versions and still routes live.
3. Dead pid, missing/refused socket or a hello naming another session: rename the registry to a unique hidden tombstone, verify the captured identity, then delete; restore a changed registry with a create-only hard link and fail closed if cleanup races; remove its socket only when the pid is dead, then continue. Unreadable registry, oversized socket path, timeout/malformed hello or missing capabilities: fail closed with exit 6 **before creating a job**, with install/restart guidance (shorten GLEN_HOME for the path case).
4. Without a live endpoint, a held glen lock selects queued RPC, preserving W2 queueing. Otherwise probe the owner lease: free selects RPC; held refuses with `run glen install-extension, then restart that omp session` when absent, or `restart that omp session so it loads the glen extension (only TUI sessions accept glen messages)` when installed. Unknown refuses.
5. Only RPC/new resolves omp. Its worker re-checks the endpoint before the pre-spawn lease probe and again **endpoint-only** immediately before prompting. An answering or unhealthy endpoint fails no_route without a prompt; no RPC-to-live handoff is attempted. Startup recovery can claim the child's own lease, so no second lease probe is safe.

**Residual race.** The extension closes the idle-TUI gap for resumed and brand-new TUI sessions before their first write. An unextended TUI can still open between a lease probe and first RPC write; the RPC path narrows that race and reports a detected fork as exit 11. `show`/`tail` retain W1's brief read-only lease probe and never route. Endpoint-only targets cannot fall back to RPC until a transcript exists.

### Controls (W3b, AC-351–369)
`abort` (visible alias `stop`), `model`, `thinking`, and `compact` route endpoint → lock-holding RPC job → unowned short-lived RPC. They follow W4's sandbox order: refuse_unreachable, live endpoint, then refuse_rpc. A responding endpoint plus a held glen lock, an unhealthy endpoint, or an owner lease held/unknown without an endpoint refuses exit 6 before a control file or process is created. Old extensions get details.reason extension_outdated, ext_version, and install/restart guidance. A free, unowned idle abort returns route none and creates nothing.

The RPC client acquires and marks the glen lock, re-follows continuations, checks endpoint and lease immediately before spawn, and reroutes once if an endpoint appeared. It starts `omp --mode rpc --no-ui [--profile P] --resume FILE`, GLEN_MANAGED=1, in the session cwd when it exists. Glen owns the child before starting the RPC handshake; startup cancellation or failure terminates its process group with TERM then KILL, reaps the leader and confirms the group is empty before releasing the lock. A baseline id mismatch is control_failed/session_moved before any operation. Re-read state after the op, report forked exit 11, close omp, then release the lock. Child stderr is piped and drained; successful controls include up to 16 omp_stderr warnings, each capturing at most 1024 bytes of a line, without writing to glen's stderr. glen never appends the transcript. Persistence error notices produce persistence_error warnings. The extension-less TUI race remains: before its first write it may hold no lease, retain an old in-memory model, and later write beside a control entry; equipped TUIs close this window.

`models` never resumes a session: use live list_models or its owning job; otherwise start a throwaway `--no-session` client and obtain current selection from the active disk branch. Unreachable sessions warn session_unreachable; outdated extensions warn extension_outdated. Endpoint-only fallback has null selection/current_source and not_yet_persisted. Without a target current is null and default_model names the in-memory default. Lists are sorted, provider-filtered before total, and create no GLEN_HOME except output spill. Sandbox refusals still follow W4.

Every command has a positive --timeout DUR, default 90s. Hello ≤2s, live abort ≤4s (extension settle ≤3s), other live ops use remaining time. RPC ready ≤min(60s, remaining); list/model calls ≤30s, thinking/state ≤10s, compact bounded by the command. Deadline/SIGINT stops an RPC process group with TERM/KILL, never an abort queued behind compact, and exits within 5s. Live stops close the socket with a may-still-finish hint; job stops report unknown outcome and the job handle. Re-route once after job_ended.

## §A5 Reply channel (`glen_reply`)
- **Consumption identity, omp 18.14.0:** `CA` below means `packages/coding-agent/src` in the read-only reference clone. `CA/extensibility/extensions/types.ts:2062` declares `sendUserMessage(...): void`; `loader.ts:349-350` and `runner.ts:1597-1598` forward that void call, so no returned message/entry id or promise is available. `CA/modes/controllers/extension-ui-controller.ts:1494-1499` catches the asynchronous session submission error and only reports it to the TUI; the extension receives no submission-error event, so glen uses the delivery timeout. `CA/extensibility/extensions/types.ts:1211-1218` exposes a detached `message_end.message`, mirrored by `lifecycle-mirror.ts:142-152`. The user message type (`packages/ai/src/types.ts:1188-1205`) has content/timestamp but no entry id. Therefore glen uses exact user text plus pending-injection order and the observed event sequence; identical texts consume one pending request at a time. This cannot distinguish an independently typed identical message, a limit of omp's exposed identity, but unrelated runs and substring matches cannot consume a request. Input boundaries arrive as start/end pairs (`packages/agent/src/agent-loop.ts:1154-1158`); queued user content preserves its text (`CA/session/agent-session.ts:9480-9484`), and explicit send delivery queues before returning (`10299-10304`).
- **Tool** `glen_reply`, params `{text: string, final?: boolean}`. Description: "Send a message back to the agent that contacted you through glen (Claude Code or Codex). Use it to answer a glen request or report progress." It returns `"delivered"`.
- **Exactly one provider per omp process.** omp rejects a host tool whose name an extension already registered.
  - glen's RPC children run with `GLEN_MANAGED=1`; the extension skips its tool and socket there, and the worker supplies a host tool via `set_host_tools`.
  - TUI and print sessions with the extension installed use the extension's tool.
- **Delivery:**
  - RPC: append in call order to `jobs/<id>/replies.ndjson`, then the inbox, and include the same records in `result.json.replies`. A job append error returns a tool error without an in-memory record; inbox errors are logged and the job reply is retained.
  - Live: the extension streams a `reply` event to a connected worker (when a glen request is active) and appends to `inbox/<sessionId>.ndjson`: `{v:1, id, session_id, job_id|null, request_id|null, tool_call_id|null, text, final, at}` — every field is always present; absent values are `null`, never omitted.
- Extension replies reach the inbox only through the extension; the Rust live worker writes job replies/results only. The extension tool does synchronous private file I/O and never waits for a peer or performs network I/O. Registry refresh is best-effort and logged through trace/TUI notification; its failure cannot prevent inbox append or successful tool delivery. If both inbox append and streaming fail, it returns isError.
- Human `glen_reply` call rows summarize reply text as `↳ glen_reply "…" · ✓ delivered`, truncated to width; JSON retains the existing raw-arguments contract (**AC-327**, O8).
- **Validation:** text must be a string, nonempty after trimming, at most 65,536 UTF-8 bytes; optional `final` must be boolean. Invalid calls record nothing and return a tool error. `final:true` is data only, not completion. W2 writes the private inbox; reading it ships in W3.
- **Reading:** `glen inbox` shows unread messages, tracked by a per-session byte-offset cursor in `inbox/.read/<sessionId>`, and marks them read unless `--peek`. `--follow` streams NDJSON.

## §A6 glen state (`GLEN_HOME`)
```
$GLEN_HOME            default ~/.glen; must be absolute; leading ~ expanded; relative → invalid_env (exit 2)
├── run/live/<lock_key>.json, run/live/<pid>.sock    W3 live endpoints
├── run/locks/<key>.lock                        W2 per-session request lock
├── jobs/<jobId>/{job.json,status.json,result.json,worker.log}   W2
├── jobs/<jobId>/replies.ndjson, control/<name>.{json,done.json}   W2
├── inbox/<sessionId>.ndjson                                    W2/W3 writes
├── inbox/.read/<sessionId>                                     W3 reads
└── out/<unix-ms>-<command>-<pid>.{json,txt}                      W0 spilled output
```
- **Lock keys:** a plain token session id is its own key; other ids use `h-<xxh64(id,0)>`. These are glen locks, never omp append gates. Lock files are never unlinked.
- **Modes:** directories glen creates are 0700, files 0600. glen never chmods a GLEN_HOME that already exists; doctor warns about group/other bits.
- **Atomic writes:** a temp file in the same directory, then rename.
- **Inbox cursors:** `inbox/.read/<stem>` contains `{v:1,offset,ino}` (0600; .read 0700). Restart from zero on inode change or truncation. Only complete lines are consumed; malformed lines count as skipped. Commit cursors only after successful output and flush.
- **Socket paths:** counted in UTF-8 bytes, paths ≥104 on macOS or ≥108 on Linux are refused. Install still succeeds with socket_path_too_long; doctor warns; send fails closed. Shorten GLEN_HOME.
- **Retention:**
  - `out/` files older than 7 days are pruned on every spill.
  - Only terminal jobs (including lost) whose parsed status `updated_at` is more than 7 days old are pruned when a new job is created. Active or unreadable jobs are kept; pruning is best effort.
  - Inbox files are kept.

Lock holder records (W3b) are written on the held inode with ftruncate/pwrite: `{v:1,kind:"job"|"control",job_id:null|string,pid,start:null|string}`. Trust only a held lock plus a matching live pid/start; job holders must belong to that session and remain non-terminal. Poll up to 1s, then use a single active, non-waiting, live job as the older-worker fallback. Unknown/control holders make other controls wait with 200ms polls; abort returns busy. Lock files are never unlinked.

## §A7 Jobs
- **Id:** glen uuidv7; **handle:** `j` + last 7 hex. `wait` accepts a full uuid or `j` plus at least 7 hex suffix digits, case-insensitively. Multiple matches return `ambiguous` with candidates; bad shapes return usage with a `glen jobs` hint.
- **`job.json`:** `{v,id,handle,created_at,route,session_id?,session_file?,session_handle?,deliver_as,text,cwd,omp_bin?,live_socket?,live_pid?,profile_flag?,owners_dir,job_timeout_ms,deadline_at}`. Routes are `rpc|new|live`. `omp_bin` is present only for RPC/new; live jobs freeze `live_socket` and `live_pid`. `owners_dir` and the applicable paths are frozen at send. Optional disk fields are omitted when absent; readers tolerate unknown fields.
- **`status.json`:** `{v,status,updated_at,worker_pid?,worker_start?,omp_pid?,request_id?,actual_session_id?,actual_session_file?,delivered_as?,waited_ms?,waiting_for_lock?,forked?:{from,to},error?:{code,message,hint?,details?},warnings:[]}`.
- **`result.json`:** `{v,prompt_status?,agent_invoked,assistant_text?,replies:[],command_output:[],stop_reason?,session_settled,settled_at?}`. Result is published before terminal status.
- **Queue display:** `waiting_for_lock:true` is published only after the blocking acquire encounters contention, and cleared on acquisition before omp startup or when cancellation/timeout ends the wait. `waited_ms` retains the acquired lock’s wait duration. Human observers show "queued behind another glen job" only while that marker is true, "starting omp" otherwise, and "waiting for the reply" after acceptance.
- **Statuses:** `queued → accepted → completed|aborted|failed`, and `queued|accepted → lost`. A worker checks terminal state before claiming.
- **Lost identity:** readers compare the pid and process start time (Linux `/proc/<pid>/stat` starttime; macOS `proc_pidinfo` start timestamp). A gone process, zombie or mismatched token persists `lost`. Unknown identity is not evidence of loss. With a null pid only, status older than 10 s exceeds spawn grace. A live matching worker is never marked lost. Loss detection re-reads status under the same status lock used by worker publications, preserving any newer terminal outcome.
- **Unsupported platforms.** Only Linux and macOS provide process start tokens. On other Unix, or when a token cannot be read, a worker records its pid without `worker_start`; readers conservatively cannot mark that job lost, even if it later dies. The null-pid grace does not apply to a recorded pid. This limitation is unsupported-platform behavior, not a fallback to the null-pid grace.
- **Deadline:** `GLEN_JOB_TIMEOUT` is parsed at job creation (1ms–24h, default 60m); invalid/zero values warn and use 60m. The recorded absolute deadline never resets. Before a result, timeout aborts and fails with `timeout`; after a completed result but before settlement it completes with `session_settled:false` and warning `settle_timeout`.
- **Job error codes:** `no_route`, `timeout`, `cancelled`, `aborted`, `omp_error`, `omp_exited`, `omp_unavailable`, `reply_tool_rejected`, `session_moved`, `session_switched`, `not_delivered`, `rpc_error`, `protocol`, `io`, `spawn`, `lost`, `internal`. `not_delivered` (live route): the request was acknowledged but omp never consumed it — the delivery deadline passed or a queued message was removed before delivery; the job fails with exit 7 and never adopts an unrelated run.
- **Clarifications (W2 conformance, 2026-10-30):**
  - *Never-ready omp vs the job deadline.* Whichever bound fires decides the code: the job deadline (`GLEN_JOB_TIMEOUT`) → `failed` `timeout`; the 60 s ready bound with deadline time left → `failed` `omp_unavailable`.
  - *Job handle shape first.* `glen wait <arg>` validates the handle shape before any lookup: a malformed argument is exit 2 `usage` even when `GLEN_HOME` does not exist; a well-formed handle with no `GLEN_HOME` or no match is exit 3 `not_found`.
  - *Lost detection age.* "older than 10 s" is measured on `status.json`'s `updated_at` field (the authoritative clock), not the file mtime.
  - *Abort bound.* With no control on omp, the worker publishes aborted within 1 s (tests allow 0.5 s scheduling slack). An in-flight control occupies omp's serial loop; abort queues behind it and W2 escalation bounds completion at 13 s (10s ABORT_GRACE, then 1s KILL_GRACE). No new control is dispatched after stop.
  - *Worker cwd.* omp runs in the session header's `cwd` when it is an existing directory; otherwise in the `--cwd` directory when one was given; otherwise in glen's process cwd.
- **Exit mapping:** completed → 0 (11 when forked); failed `no_route` → 6; other failed/aborted/lost → 7 `job_failed` with `{status,job,error,forked}` details. A bound expiring while queued/accepted → 10 with `ok:true` and a running document. Foreground SIGINT prints that document with `interrupted:true` and exits 130; worker/omp continue. Worker SIGTERM/SIGINT/SIGHUP abort with `cancelled`.
- **Control seam (W3b):** atomically drop `jobs/<id>/control/<name>.json` = `{v:1,op,args,deadline_ms}` (Unix ms). Replies are `{v:1,ok:true,data?}` or `{v:1,ok:false,error:{code,message,details?}}` at `<name>.done.json`, then remove the request. Dotfiles/results are ignored. The polling loop is 50ms during a run; startup/lock wait defers non-abort ops until a client exists. The worker runs list_models, set_model, and set_thinking_level one at a time off that loop on its held client, with 30s list/model and 10s other budgets bounded by the job deadline. Expired requests answer timeout unrun. Compact answers busy. Pending ops at job end answer job_ended; an in-flight request is skipped until its actual result arrives after scope join, preventing duplicate application. Abort acknowledgement carries data.job_status and session_id/queued. Startup cancellation publishes terminal status/ack before joining the bounded startup thread.
- **Live job semantics:** no glen session lock and no omp child. The existing detached worker and wait/jobs lifecycle apply. Ack publishes accepted and delivered_as; reply events append jobs/replies and result in order. Every failure after send writes result before status. Settled completed/aborted/error maps completed/aborted/failed omp_error; shutdown or EOF fails omp_exited; ack mismatch fails session_moved with from/to details. Unknown events are ignored. Ack has a 10s bound (protocol/delivery_unknown), and the earlier absolute deadline is timeout. A post-ack deadline does not interrupt omp; the hint distinguishes consumed from still queued. Worker signals abort observation with cancelled and live_turn_continues. Live control abort returns unsupported_op with “stop the run with `glen abort <session>`”; other control files explain the socket route, and the job continues. Send-frame writes have ≤200ms cancellation checks and a 2s total budget.

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
| `glen show <session> [--cwd DIR] [--thinking] [--tools collapsed\|full\|none] [--raw]` | Active-branch transcript; `--raw` = entries as JSON; `--cwd` scopes session resolution | W1 | W1 |
| `glen tail <session> [--cwd DIR] [-n N] [-f] [--thinking]` | Last N transcript items; `-f` follows (NDJSON in JSON mode); `-n 0` gives no backlog; `--cwd` scopes initial resolution | W1 | W1 |
| `glen send <session\|@latest\|@new> <message\|-> [--as auto\|steer\|follow-up] [--steer\|--follow-up] [--wait DUR\|--no-wait] [--cwd DIR]` | Send through the live route or RPC, then wait up to the bound (default 90s) | W2 (RPC), W3 (live) | W2 |
| `glen wait <job> [--timeout DUR]` | Keep waiting for a job (default 90s) | W2 | W2 |
| `glen jobs [--session S] [--active] [-n N]` | List glen's jobs | W2 | W2 |
| `glen abort <session> [--timeout DUR]` (alias `stop`) | Stop the active turn; report queued glen jobs | W3b | visible |
| `glen models [session] [--provider P] [--timeout DUR]` | List exact provider/model pairs and session selection | W3b | visible |
| `glen model <session> <spec> [--timeout DUR]` | Switch model/provider, session scope | W3b | visible |
| `glen thinking <session> <level> [--timeout DUR]` | Set supported thinking level, session scope | W3b | visible |
| `glen compact <session> [--instructions TEXT] [--timeout DUR]` | Compact an idle session | W3b | visible |
| `glen inbox [--cwd DIR] [--session S] [--peek] [--all] [-f]` | Messages omp sent back with `glen_reply`; `--cwd` selects one project's sessions | W3 | W3 |
| `glen watch [<session>] [--all] [--cwd DIR]` | Full-screen sessions, transcript, replies and prompt; Tab changes focus, arrows navigate, Enter sends from the prompt, Ctrl-T cycles delivery, G follows, q quits from a pane | W5 | visible |
| `glen install-extension [--dry-run] [--force] [--uninstall]` | Install the bundled omp extension for the effective agent dir/profile | W3 | W3 |
| `glen install-skill [--claude] [--codex] [--project] [--dry-run] [--force] [--uninstall]` | Write SKILL.md for Claude Code and/or Codex; defaults to both, --project means cwd | W4 | visible |
| `glen doctor` | omp binary/version, paths, glen home, extension and endpoints, .env, skills, sandbox | W0 (extended W3/W4) | visible |
| `glen completions <bash\|zsh\|fish\|elvish\|powershell>` | Shell completion script (raw), directory hints for --cwd | W0 | visible |
- **Send rules:** `--steer` / `--follow-up` alias `--as` and conflict with explicit `--as` or each other. With `@new`, `--cwd` sets the new session's directory, which must exist. For every existing target (handle, id, prefix or `@latest`), it scopes resolution to that project's bucket; `@latest` means that bucket's newest session. A match found only elsewhere is `not_found` with `details:{reason:"outside_cwd",cwd:<absolute DIR>}` and a hint naming its project. An explicit file path bypasses the bucket filter and is resolved against the process cwd. omp runs in an existing recorded session cwd; only a missing recorded directory falls back to `--cwd`, then to glen's process cwd (AC-1007–AC-1012, superseding AC-209's selector restriction). Message `-` reads stdin; empty, non-UTF-8 or more than 512 KiB is usage. RPC auto/follow-up sends `streamingBehavior:followUp`, steer sends `steer`; idle baseline reports `delivered_as:prompt`.
- **Install:** installed/unchanged/upgraded/reinstalled/replaced/removed/not_installed/skipped; unchanged leaves inode/mtime untouched. Marker line is `// glen-extension version=<v> protocol=1 digest=<xxh64-seed0-body-hex16>`. A newer, edited or foreign file is skipped with extension_newer/modified/foreign unless --force; uninstall also protects modified/foreign files. --dry-run reports the action without any writes. Uninstall removes only index.ts and the empty glen dir (a sibling is preserved with extension_dir_not_empty). It never creates GLEN_HOME. Human output explains the tool and uninstall command, plus restart guidance on changes. For replace/upgrade (install-extension and install-skill alike), glen publishes with an atomic exchange (`renameat2(RENAME_EXCHANGE)` on Linux, `renameatx_np(RENAME_SWAP)` on macOS), then checks that the displaced file is the one it inspected (dev, ino, xxh64 digest) before deleting it. A file another writer put there after glen's last check is swapped back and left in place, and the install reports `skipped` with `changed_during_install`; if even the swap-back cannot be verified, the displaced copy is kept and its path named in the error. On a filesystem with no atomic exchange (EINVAL, ENOTSUP/EOPNOTSUPP, ENOSYS) glen falls back to a plain rename and warns `replace_not_atomic`; only then can a replacement made in that window be overwritten. Any other swap error fails the install without falling back (AC-1017–AC-1019). glen never writes the extension file through a symlink.
- **Inbox:** --session matches full id or ≥4 hex tail, case-insensitive; --peek leaves cursors untouched; --all includes read records. With --cwd, filter by the catalog and registry project bucket; session handles and prefixes use the same matcher as show, and --session intersects the project scope. If only foreign stems match, non-follow output warns outside_cwd. Unclassified replies stay unread; without --cwd, the inbox aggregates all projects. Follow rescans once per poll when an unclassified stem appears, including sessions created after startup. Sort by parsed at, then stem/file offset; malformed timestamps sort last. Missing home/inbox is empty and lazy. -f always emits NDJSON, polls every 250ms including new files, and commits after successful message output; SIGINT writes end and exits 130.
- **Global flags:** `--json`, `--human` (mutually exclusive), `--color auto|always|never`, `--profile NAME`, `-h`, `-V`.
- **Defaults and aliases:** `--color` defaults to `auto`; `--profile` overrides `OMP_PROFILE` and `PI_PROFILE`. `ls` and `jobs` use `-n/--limit` (default 20); `tail` uses `-n/--lines` (default 10). `tail` and `inbox` accept `-f/--follow`. `show --tools` defaults to `collapsed`; `send --as` defaults to `auto`; `send --wait` and `wait --timeout` default to `90s`. `-h/--help` and `-V/--version` are available on every command. Unfinished commands remain hidden and return `not_implemented` (exit 8).
- **Durations:** `<n>ms|s|m|h` or bare seconds, at most 24h. `--wait 0` equals `--no-wait`. `ls --since` accepts `<n>s|m|h|d|w` or bare seconds, at most 3650 days.
- **Internal:** `glen __worker <job-id>` (W2), always hidden.
- **Considered and rejected:**
  - a `glen reply` CLI: the reply path is the `glen_reply` tool, and an omp Bash call has no job context;
  - a separate `glen steer` verb: folded into `send --as`;
  - a `--new` flag: an optional first positional is ambiguous, so it became `@new`;
  - an MCP server: not needed for v0.1.0.

**Excluded controls:** auto-compaction, steering/follow-up/interrupt queue modes and auto-retry are session-only RPC overrides with persist=false and have no equivalent extension setter. Cycle model/thinking duplicates explicit selection. Service tier/fast mode is provider-specific and deferred. Clearing queued messages has no extension equivalent. Thinking auto/inherit is a documented v0.1.0 gap. Cancelling queued glen jobs remains outside this wave. W4's already-visible install-skill remains available; watch is unchanged.

## §A10 Output contract
`docs/json.md` is the normative per-command contract; this section keeps the rules.
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
- **Doctor data:** `data.glen` contains `version`, `home` and `home_exists`; `data.omp` contains `bin` (path or null), `version` (string or null) and `tested_version` (`18.14.0`). `data.paths` contains the §A12 resolved paths, `profile` (name or null), `agent_dir_source` (`default|profile|env`) and `sessions_dir_source` (`agent_dir|xdg`). `data.checks` is an ordered array of `{name,status,value,detail}`, with status `ok|warn|fail|info`. `value` is the raw absolute path or check value; `detail` is the reason for the status, or an empty string when there is none. The checks are `omp_binary`, `omp_version`, `profile`, `agent_dir`, `sessions_dir`, `glen_home`, `extension`, `live_endpoints`, `omp_env_files`, `skill_claude`, `skill_codex`, `sandbox`. `data.extension={path,state,version,current}` describes current/not_installed/outdated/newer/different_home/modified/foreign. live_endpoints reports N live, M stale by pid, warning on unreadable registries or socket path limits. `data.skills` contains claude/codex `{path,state,version,current,project}`, with project null or `{path,state,version}`. W4 rows are advisory, never fail. An untested version succeeds with warning `omp_version_untested`. A failed doctor returns `omp_unavailable`, with the same data under `error.details.report`, and its human table before the error line. Missing directories are reported without creating them.
- **Install-skill:** `{version,dry_run,scope,targets:[{agent,path,action,state,installed_version}]}`; state/version describe the pre-run file (null version when absent). Both agents are selected by default; flags select one or both. User targets: `$CLAUDE_CONFIG_DIR/skills/glen/SKILL.md` (nonempty, relative to cwd), else `~/.claude/skills/glen/SKILL.md`; Codex `~/.agents/skills/glen/SKILL.md`. Project targets are cwd `.claude/skills/glen/SKILL.md` and `.agents/skills/glen/SKILL.md`. Directories are 0755 before umask; files atomic 0644 before umask; glen never chmods past the umask. No GLEN_HOME is created. Inspection of all selected targets precedes writing. Destination and ancestor identities (dev/ino and file digest) are revalidated before rename/unlink; a concurrent change skips with `changed_during_install`, including under --force. Identical files stay untouched; intact older/same-version other builds upgrade, newer/modified/foreign skip unless --force. Directory symlinks below the scope root always skip; links at/above it are followed. Uninstall removes only the file and empty glen directory. Dry-run changes nothing. IO errors include `details.done` and collected warnings.
- **W4 cycle 1 clarifications:** `omp_env_files.value` is an array of absolute paths (§A10 wins over AC-409 tilde wording). A stale digest means `modified` even for an older version; user edits require --force. Another agent or foreign marker has `installed_version: null`. Uninstall skips modified/foreign files with the same warning code as install. Project installs need no HOME; user installs with HOME unset return `invalid_env`. Only non-empty `CODEX_SANDBOX` marks a Codex sandbox (empty is unset); `CODEX_SANDBOX_NETWORK_DISABLED=1` alone permits send and maps actual permission failures to exit 6 `sandboxed`. The doctor sandbox row for that case is "network disabled by the host; escalated commands still work" everywhere except Linux, where any marker hides omp's lease — see the W10 clarification in §A13 for the Linux rule and its own doctor row.
- **W4 cycle 2 clarification:** warning code `changed_during_install` means a destination or ancestor changed after inspection and was skipped, even with --force. A stale digest means `modified` for older, same and newer versions; user edits always win and skip without --force. Foreign/other-agent markers have both `installed_version: null` and doctor `skills.<agent>.version: null`. With either Codex marker (non-empty CODEX_SANDBOX or CODEX_SANDBOX_NETWORK_DISABLED=1), permission/read-only failures writing GLEN_HOME, including spill and job writes, map to exit 6 `no_route` with `details.reason:"sandboxed"`; without a marker they remain `io`.
- **W1 reader documents.** `ls` returns `{scope:{all,cwd,bucket},sessions_dir,total,total_all,sessions:[]}`. Scope cwd/bucket are null with `--all`; total is after filtering but before limiting. Each session has `handle,id,path,bucket,cwd,title,first_message,created_at,updated_at,age_ms,size_bytes,model,last_turn,state,state_source,state_detail,continuation_of,parent_session`; nullable fields remain present as null. Future mtimes saturate age_ms at zero (human: `just now`).
  - `show` and document `tail` return `{session,continued_from,leaf_id,skipped_lines,items:[]}`. `show --raw` replaces items with active-branch `entries:[]`, including unknown entry kinds. `tail -n 0` returns an empty items array.
- **W2 send/wait documents:** completed data is `{status:"completed",job,session:{id,handle,path},assistant_text,replies,command_output,stop_reason,session_settled,delivered_as,forked}`. `forked` is null or `{from,to,to_handle}`. RPC `session` identifies the resolved target after following continuations (the session the job was sent to, e.g. C when B continued in C); the new route uses the actual session identity. Nullable output fields remain present.
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
- **W3 documents:** inbox returns `{total,marked_read,skipped_lines,messages:[record+session_handle+job_handle]}`; inbox -f emits `message {message}`. install-extension returns `{action,path,version,previous_version,glen_home,profile,agent_dir,agent_dir_source,dry_run,restart_required}`. Both use exactly one envelope in JSON document mode and empty stderr. `ls` queries endpoints concurrently (300ms each; detached threads, 1s overall cap), skips owner probes for matching extension rows, and never deletes a registry or creates GLEN_HOME. Endpoint-only ls paths are null.
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
  - Line-oriented command output goes through `anstream::AutoStream`.
- **Depth:** `COLORTERM` truecolor/24bit → 24-bit; `TERM` containing `256color` → 256; otherwise 16.
- **Theme:** `GLEN_THEME=dark|light`, then the last semicolon-separated integer in `COLORFGBG` (0–6 or 8 → dark; 7 or 9–15 → light), then dark.
- **Full-screen mode.** `watch` requires interactive stdin and stdout, refuses JSON mode and `TERM=dumb`, and returns `usage` (exit 2) before touching the terminal when the gate fails. Ratatui cells use foreground colours only, with no background fills. The same colour precedence applies: when colour is disabled (including by `NO_COLOR`), modifiers such as bold and dim are dropped too. `GLEN_THEME=light|dark` selects the full-screen palette.
- **Palette.** Foreground only; body text keeps the terminal's default colour. Sourced from the Nodex redesign content tone (research `93cec9de` rev 2). The values live in `src/render/palette.json`. `palette_contract` (tests only) checks them against `style()`, and the site reads the dark values at build. This table mirrors that file:

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
- **Write failures (AC-1001/AC-1002, amending AC-029).** When a stdout write or flush fails with EPIPE (the reader closed the pipe), glen writes nothing more to stdout or stderr and keeps the outcome's own exit code: 0 for success, the error's code for a failure envelope, or 10, 11 or 130 for a running, forked or interrupted outcome. This applies to every command and mode, including JSON, human, raw and clap help/version/usage. Any other stdout failure exits 1, without a panic or a second JSON document. A failed stderr write also exits 1; clap usage successfully written to stderr keeps exit 2 regardless of stdout.
- **Lazy state.** `--help`, `--version`, `doctor` and path resolution never create `GLEN_HOME`. Directories are created on first write (spill in W0; jobs, inbox and live endpoints later).

### W3b control documents
`docs/json.md` defines the full five documents. Each carries session `{id,handle,path}` (path null before persistence) and route. Model returns model/previous/changed/thinking_level/thinking_levels/busy/scope/forked; thinking returns requested/previous/changed/model/available/busy/scope/forked; compact returns compacted/tokens_before/tokens_after/forked; abort returns was/aborted/idle_after/queued/queued_jobs (job route adds job/job_status). Forked is null or `{from,to,to_handle}`. Endpoint-only changes warn not_yet_persisted. ls still reports the last answering model, which may differ from models current until the next turn.

Human forms name the session and route: “nothing to abort”, “stopped the running turn”, provider/id with thinking level, or compacted token counts. Models renders a COLUMNS-bounded MODEL / NAME / CONTEXT / THINKING table, truncating NAME first and marking current. omp-sourced strings are sanitized only for presentation.

control_failed exits 7: no_auth (log into provider), busy (abort first or wait), omp_error (omp's message), timeout, session_moved, job_ended, protocol. invalid_level is usage/2; model_not_found is not_found/3; model ambiguity is 4. Exact provider/id wins over case-insensitive pair/bare-id resolution. The extension supplies all available models; RPC may apply its enabled-model filter. Same-version installed extensions with an intact digest, matching baked GLEN_HOME and older body are outdated (“older build”), upgraded by install-extension and warned by doctor; a different home remains reinstalled.

### W3 conformance clarifications
- `state_source` is `"extension"` for endpoint-only and extension-backed sessions everywhere, including `ls`, `show` and `tail`. This supersedes O3's `"endpoint"` wording. An unwritten session has `path:null`.
- `install-extension.data.profile` is null for the default profile and the profile name otherwise, matching `doctor.paths.profile`. `doctor.data.extension.current` is always a boolean.
- `install-extension --force` over a newer, modified or foreign file reports `replaced`. A current file remains `unchanged` without rewriting it. An older version reports `upgraded`; a different baked home reports `reinstalled`. Symlink refusal cannot be overridden.
- Omitted reply `final` is stored explicitly as `false`; the key is always present.
- AC-321's TUI notice is once per failing `session_start`, so a later start gets its own notice.
- AC-326's endpoint-only `tail -f` waits for the transcript to appear, then streams it. If the TUI quits without writing, it ends with `not_found` after the five-second disappearance grace.
- `inbox --session` rejects hex suffixes shorter than four digits with exit 2, `usage`, in both normal and follow mode. Full session ids remain accepted.
- Adjacent queued user messages consumed in the same run with no intervening assistant output share a batch reply span. Every job receives the same assistant text and reply records, and a `batched` warning names the other job handles. A user message always ends the prior span outside that batch exception. A custom/async-result/system-notice message ends the span only after it contains assistant text; before an answer, the span and its glen ownership continue past the notice. The extension writes one durable inbox record per tool call; shared records have null single-owner `job_id` and `request_id` and are streamed identically to every batch member. A new run or intervening assistant output separates the spans.

## §A11 Exit codes

A closed stdout pipe (EPIPE on write or flush) preserves the outcome's code below, including failure codes such as 2 (`usage`), 3 (`not_found`), 5 (`omp_unavailable`) and 6 (`no_route`), and statuses 10, 11 and 130. It does not turn failures into success: glen stops without further stdout or stderr output. Other stdout failures and failed stderr writes exit 1. Clap usage successfully written to stderr keeps exit 2 regardless of stdout (AC-1001/AC-1002).

| Exit | `error.code` / status | Meaning |
|---|---|---|
| 0 | — | success (`ok:true`) |
| 1 | `internal`, `io` | unexpected failure |
| 2 | `usage`, `invalid_env` | bad flags or arguments; bad env value (`OMP_PROFILE`, `PI_PROFILE`, `GLEN_HOME`, missing `HOME`) |
| 3 | `not_found` | session, job or handle not found |
| 4 | `ambiguous` | handle matches several; candidates in `details` |
| 5 | `omp_unavailable` | omp missing, unrunnable or version unparseable |
| 6 | `no_route` (foreground or worker-side refusal) | live owner without a glen endpoint, probe unknown, or a sandbox that cannot reach omp (`details.reason:"sandboxed"`); refusing to fork |
| 7 | `job_failed`, `control_failed` | failed/aborted/lost job, or a control failure with details.reason |
| 8 | `not_implemented` | subcommand declared but not in this build |
| 10 | (ok) `running` | wait bound elapsed; job continues; use `glen wait <job>` |
| 11 | (ok) `forked` | omp moved the session to a continuation; use the new handle |
| 101 | — | watch panic; terminal teardown runs before the panic report |
| 129 | — | watch received SIGHUP |
| 130 | — | interrupted (SIGINT), including watch; send/wait print a running document with `interrupted:true`; a detached job keeps running |
| 143 | — | watch received SIGTERM |

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
9. **Known gap.** omp fills *unset* `PI_CODING_AGENT_DIR`/`XDG_*_HOME` from `.env` files (cwd `.env`, `<agent_dir>/.env`, `<config_root>/.env`, `~/.env`; `utils/src/env.ts:289-316`). glen reads only its own environment. Doctor’s `omp_env_files` warns when these files assign PI_CODING_AGENT_DIR, OMP_CODING_AGENT_DIR, XDG_DATA_HOME or XDG_STATE_HOME while the corresponding glen variable is unset/empty. Unreadable files are skipped; OMP_CODING_AGENT_DIR maps to PI_CODING_AGENT_DIR.

- **W4 cycle 1 path clarification:** every §A12 path variable, including `CLAUDE_CONFIG_DIR`, treats whitespace-only values as unset. `.env` detection checks PI_CODING_AGENT_DIR and its OMP_CODING_AGENT_DIR alias, plus XDG_DATA_HOME and XDG_STATE_HOME.

- **W4 cycle 2 clarification:** the OMP_CODING_AGENT_DIR alias also applies to glen's own process environment: when PI_CODING_AGENT_DIR is unset/empty, a non-empty OMP_CODING_AGENT_DIR counts as set for path resolution and suppresses dotenv warnings for either spelling.

- **Environment handling difference.** glen drops non-UTF-8 environment keys/values and treats whitespace-only path values as unset; omp uses JavaScript strings and treats whitespace-only values as set. Profile names are still trimmed. `NO_COLOR` and `CLICOLOR_FORCE` use literal values as specified in §A10.

- **omp executable:** `GLEN_OMP_BIN` (non-empty) accepts an absolute file path or a bare executable name searched on PATH; otherwise glen searches PATH for executable `omp`. `doctor` runs `--version` with a 10 s deadline. The first stdout line must be `n.n.n`, optionally prefixed by `omp/`. Failure, timeout or unparseable output is `omp_unavailable`.

## §A13 How Claude Code and Codex use glen
- The template is `skill/glen/SKILL.md`, embedded with include_str; edit it there. Claude and Codex bodies are byte-identical. Frontmatter starts on line 1; the version/agent/xxh64(seed 0) digest marker follows its closing line. A digest covers every byte except the marker line and its newline.
- Line/block `<!-- glen:if NAME… -->` guards require every named visible command or visible alias. Block guards close with `<!-- glen:end -->`, cannot nest, and disappear from output. This lets W4 merge before W3/W3b. The drift test checks commands, flags, enums, aliases, coverage, numeric defaults and exit codes against clap; the JSON contract test checks real fixture/fake envelopes.
- Verified 2026-10-30: Claude Code 2.1.309, Codex CLI 0.166.0. Recheck these facts after upgrades.
  - Claude personal skills: `$CLAUDE_CONFIG_DIR/skills` (nonempty), else `~/.claude/skills`; project `.claude/skills` from cwd up to repo root.
  - Claude frontmatter is parsed only with `---` on line 1: `name`, JSON-quoted `description`, `allowed-tools: Bash(glen *)`. The legacy `Bash(glen:*)` is equivalent; W4 deliberately ships the current space form. Malformed YAML loses the fields. Listing descriptions are cut at 1536 chars.
  - allowed-tools grants permissions for the invoking turn only. Run one plain glen command; compound commands require approval of every part. A general-purpose subagent inherits CLAUDE.md unless omitClaudeMd is set; acceptance (b) audits that context and (b3) tests clean discovery.
  - Codex user skills: `$HOME/.agents/skills`; repo `.agents/skills` from cwd to repo root; admin `/etc/codex/skills`. `$CODEX_HOME/skills` is deprecated. Name limit 64, description required, one line, limit 1024 chars. Unknown keys are ignored; the Codex variant omits allowed-tools.
  - Codex classic shell kills after 10 s unless timeout_ms is supplied. exec_command yields after 10 s by default (0.25–30 s), keeping the process running. Output budget is normally 10000 tokens, with a 10000-byte fallback for unknown models. Use a >=100 s shell timeout or `glen send <h> "…" --wait 5s` then `glen wait <job> --timeout 5s` while exit is 10. Never resend after exit 10, 130 or a cut-off call.
  - Only non-empty CODEX_SANDBOX (e.g. seatbelt) means inside the Codex sandbox; empty means unset. CODEX_SANDBOX_NETWORK_DISABLED=1 alone also occurs on approved escalated commands (codex-cli 0.166 sandboxing/mod.rs:169-179, spawn.rs:86-88), so send proceeds (on Linux only through the live route or to `@new`; see the W10 clarification below). Doctor reports that case as info: "network disabled by the host; escalated commands still work" (Linux: its own row, below). Sandbox sends refuse before any lease probe, after argument/target validation; network-enabled sandboxes permit live delivery only. RPC/new jobs always refuse inside a detected sandbox. Real permission failures on network/socket/job writes map to exit 6, reason `sandboxed`, with the hint, including host network-disabled-only commands. PermissionDenied and ReadOnlyFilesystem (Linux EROFS) both map. Linux network-on CODEX_SANDBOX runtime behaviour remains unverified; macOS alone was probed.
  - workspace-write denies writes under HOME, including default `~/.glen`. Spills and send job writes fail with exit 6 `sandboxed`, `glen_home_writable:false`, hinting to set GLEN_HOME under $TMPDIR or writable_roots. The inbox read marker uses the same helper. Prefer bounded tail reads and inbox --peek. Network-enabled live delivery requires writable GLEN_HOME.
  - codex:codex-rescue companion task --write uses workspace-write and approvalPolicy never; it accepts --cwd and --prompt-file. It cannot escalate from inside that run.
- Loop: ls → show/tail → send → read data/replies; exit 10/130 → wait, 11 → data.forked.to_handle, 6 → report hint and stop (for a session open in omp without the extension the hint is `glen install-extension`, then restart the omp TUI). Afterwards, `glen inbox`. Error messages/hints are diagnostic prose, not stable parsing keys.

- **W4 cycle 2 clarification:** AC-410 takes precedence over shorthand routing descriptions: argument/target validation → sandbox refusal → omp executable lookup, endpoint connect, lock and lease checks. CODEX_SANDBOX_NETWORK_DISABLED=1 without CODEX_SANDBOX still proceeds optimistically, with unknown sandbox kind. Actual EPERM/EACCES/EROFS on socket connect/bind, lease probes or GLEN_HOME writes map to exit 6 `sandboxed` with either marker; worker omp spawn/network failures (connection refused, DNS resolution failure, permission denied on connect) also use that mapping with the network-disabled marker.
- **W4 cycle 2 residual limitation:** a Linux Codex sandbox with network enabled and neither CODEX_SANDBOX nor CODEX_SANDBOX_NETWORK_DISABLED is undetectable from glen. Failures surface as ordinary `io`/`omp_error`, not `sandboxed`; this is an accepted gap.
- **W10 clarification (2026-10-31; fix cycle 2 restores the blanket rule and supersedes cycle 1's namespace comparison):** on Linux, omp's session lease is an abstract socket, which is scoped to a network namespace, and a Codex sandbox may run glen in its own. No in-process signal tells such a namespace from the host's reliably: with Codex's `--unshare-pid`, PID 1 is the sandbox's own bwrap in the same new namespace, and a new namespace can list kernel fallback tunnels (`tunl0`, `gre0`, `sit0`, …) besides `lo`. Rule: on Linux, when `CODEX_SANDBOX` is non-empty or `CODEX_SANDBOX_NETWORK_DISABLED=1`, the lease is **hidden**, whatever a probe returns (Free, Held or Unknown). `ls`, `show` and `watch` display it as `state:"unknown"`, the existing state vocabulary (an answering extension endpoint stays authoritative), and `models` adds `session_unreachable`. Every RPC resume of an existing session, by `send` or a session control, refuses exit 6 `no_route` with `details.reason:"sandboxed"` and `details.lease:"unknown"`, before the lease probe, the omp lookup and any job write; `send` refuses even while a glen job lock is held, and the worker's pre-spawn probe refuses the same way (a control addressed to an already running glen job still reaches that job). The live route (extension endpoints are filesystem sockets) and `@new` are unaffected. So on Linux `CODEX_SANDBOX_NETWORK_DISABLED=1` alone, which Codex also sets on approved escalated commands, permits only the live route and new sessions; resuming over RPC needs glen run from a terminal outside Codex. The Linux doctor row for that case says "network disabled by the host; on Linux glen will not resume an existing session over RPC here (exit 6 sandboxed); the live route and new sessions still work". On macOS (flock leases, no network namespaces) nothing changes. Verified in CI by the `linux-sandbox` job's `--unshare-net` scenarios (s7–s9, s8 also with `--unshare-pid` and with a held glen lock), never the real Codex sandbox.

Controls in the agent loop: `glen models <h>` → `glen model <h> <provider/id>` or `glen thinking <h> high`. Only apply user-authorized changes. On extension_outdated install the current extension and restart that omp session; never bypass a refusal with another omp. Compact needs an idle session; abort first or wait. Abort leaves queued omp messages, and queued_jobs warns about glen jobs that start next.

## §A14 Security and privacy
- Local only; no network.
- GLEN_HOME is 0700. The socket's only protection is filesystem permissions (a same-user boundary).
- The extension runs unsandboxed inside omp, like every omp extension. glen ships its source in the repo and installs it only on explicit `install-extension`.
- Message text is stored only under GLEN_HOME.
- glen never writes omp session files.

## §A15 Testing strategy
- W4 acceptance harness: `tests/acceptance/` (not CI), evidence `.claude/evidence/w4-acceptance/<UTC>/`. The recording wrapper guards every mutating target against its manifest and records argv, exit, duration and stdout. Person mode uses the real binary on a pty. Step 11 is deferred until W3; c2 requires owner consent and byte-identical config restoration.
- Final O1: acceptance uses a fresh cwd under $TMPDIR with normal omp credentials, never copies credentials. Only new session files in its own scratch bucket(s) may differ in the owner sessions snapshot (names/mtimes/sizes); evidence lists those buckets. Missing credentials/model access blocks setup. Extension installation alone uses a scratch agent dir; TUIs explicitly load that copy. GLEN_HOME is under $TMPDIR for sandbox writes. Evidence is gitleaks-scanned; no secrets or env values are recorded.
- Fixtures live under `tests/fixtures/` (see its README).
- W5 uses ratatui's `TestBackend` plus insta for deterministic size/theme/depth and state snapshots. Create them with `INSTA_UPDATE=always cargo test --lib tui::view::snapshots`, review every snapshot, commit it, then rerun without `INSTA_UPDATE` to check the pinned result.
- W5 real-binary tests in `tests/watch.rs` use rustix pty/termios and the `fake_omp`/`fake_ext` peers. Restoration checks require raw mode observed while watch is alive and cooked mode after exit on the same slave fd. On macOS, controlling-tty escape-teardown smokes are paired with non-controlling pty termios checks because session-leader exit revokes the controlling slave; Linux checks both on the controlling tty.
- `GLEN_OMP_BIN` points to the `fake_omp` cargo example (`tests/support/fake_omp.rs`). It supports version, RPC and `hold-lease` modes. `cargo test` builds examples; `tests/common::fake_omp_bin` rebuilds it once per integration-test process so filtered runs use the current source. It is not installed by cargo install.
- `FAKE_OMP_SCENARIO`: ok, local, progress, bad_reply, unsettled, never_settles, error, aborted, crash, hang, wedge, fork, fork_silent, moved, reject_tools, reject_filter, no_ready, delayed_state, fork_reply, local_result_unsettled, unsettled_exit, local_exit, stale_state. Timing/shape knobs: `FAKE_OMP_DELAY_MS`, `FAKE_OMP_READY_DELAY_MS`, `FAKE_OMP_SETTLE_DELAY_MS`, `FAKE_OMP_REPLY_BYTES`, `FAKE_OMP_OWNERS_DIR`, `FAKE_OMP_V2=1|partial`. `FAKE_OMP_LOG` records start, inbound/outbound frames and exits as NDJSON. `FAKE_OMP_STARTUP_RECOVERY=1` makes the fake claim its lease and append a recovery record to the copied fixture before ready. `FAKE_OMP_STATE_DELAY_MS` delays running state replies without blocking abort. `FAKE_OMP_RUN_RELEASE` holds prompt work until a test-owned file exists. `FAKE_OMP_READY_RELEASE` holds ready until a test-owned file exists (20 s bound). `FAKE_OMP_PROMPT_SUBMIT_DELAY_MS` delays the debug worker’s submission thread to exercise the cancellation gate; `FAKE_OMP_ACK_FORWARD_DELAY_MS` delays acknowledgement forwarding. Release workers ignore both. `FAKE_OMP_LEASE_PROBE_GATE` gates the debug send after the glen-lock check until a test-owned file exists (5 s bound, `<path>.ready` signals entry). `FAKE_OMP_NEVER_SETTLE=1` keeps fork scenarios unsettled; `FAKE_OMP_RESULT_RELEASE` holds the fake before its result. `FAKE_OMP_OLD_STATE_DELAY_MS` controls the stale_state scenario’s old identity response.
- Process tests wait for terminal status, worker exit and the omp group to disappear before dropping scratch directories. The startup-abort regression delays ready by 5 s and checks terminal status/ack within 1.5 s with no prompt.
- W2 Step 15 smoke is separate orchestrator evidence: use fresh scratch PI_CODING_AGENT_DIR and cwd, never resume a user session. It remains unverified in the sandbox.
- W3 `fake_ext` (`tests/support/fake_ext.rs`) serves real private Unix sockets/registries from a separate process: ok, busy, progress, slow, never_settles, aborted, error, moved, switched, shutdown, crash, mismatch, rekey, hang, no_send, v2, malformed, no_ack, queued. FAKE_EXT_LOG records inbound/outbound frames; FAKE_EXT_DELAY_MS, FAKE_EXT_VERSION and FAKE_EXT_KEEP_FILES tune scenarios; FAKE_EXT_CWD supplies catalog scope. It owns live inbox writes; worker tests check no duplicates.
- `tests/extension_ts.rs` runs extension/glen/test/*.test.ts under Node strip-types, with a no-flag fallback if Node drops that flag; missing Node fails CI and skips locally. CI installs Node 26. No npm dependencies or Bun APIs.
- W3 Step 14: **run on the host — see evidence** `.claude/evidence/v010-wave-3-live/README.md`. Steps other than busy attribution passed; Step 5 failed and motivated the O6 message-boundary amendment. `extension/glen/test/settlement.test.ts` replays all 42 trace events in order, supplementing the body-free trace with transcript bodies. The post-fix live Step 5 rerun remains a host gate; never call fake-host replay real-omp acceptance.
- An `Env` snapshot means code and tests never mutate the process environment.
- Integration tests use `CARGO_BIN_EXE_glen` with `env_clear()`.
- Conformance tests live in `tests/conformance.rs` (the `/implement` tail).

W3b fakes share `tests/support/fake_models.json` (six complete SDK ModelInfo records). fake_omp accepts --no-session, tracks selection on resumed copies, appends model/thinking/compaction entries with sync_all, and supports fork_control and compact_error. FAKE_OMP_SERIAL=1 processes ordinary commands in arrival order, including abort; FAKE_OMP_COMPACT_DELAY_MS delays compaction without writing after a stop. W2 concurrent delayed-state behaviour remains the default.

fake_ext defaults to all eight ops; FAKE_EXT_OPS=w3 advertises the historical set. FAKE_EXT_NO_AUTH, FAKE_EXT_COMPACT_DELAY_MS, FAKE_EXT_ABORT_STUCK and FAKE_EXT_POST_ID cover auth, cancellation, settle bounds and post-op forks. The TS host covers every op's happy/error path, expected identity, subagents, UI notifications, concurrent compaction and abort. tests/controls.rs covers RPC/live/job routing and output. Real-omp Step 12 remains an orchestrator-only smoke, explicitly skipped here; no runtime acceptance is inferred from fake-based results.

## §A16 Wave map
- **W0** foundation: this plan.
- **W1** disk reader: ls, show, tail and `-f`, handles, continuations, leaf rule, read-only lease probe.
- **W2** RPC driver: send/wait/jobs for sessions with no live owner, `@new`, the per-job worker, the per-session lock, the `glen_reply` host tool, fork detection, the fake RPC peer, the `omp-rpc` git dependency.
- **W3** live route: the bundled extension, install-extension, endpoint-first routing, inbox, talk-back from the TUI.
- **W3b** session controls: abort/stop, models, model/provider, thinking and idle compaction; one router/executor over live, owning job and unowned RPC.
- **W4** agent surface: SKILL.md (two variants), install-skill, extended doctor, completions polish, acceptance runs with Claude Code and Codex.
- **W5** visible `glen watch` TUI: sessions, persisted-entry transcripts, a send prompt, job activity and peek-only replies; shared follower/submit cores, worker threads, TestBackend snapshots and real-binary pty tests. Session controls (`m` model, `T` thinking, `C` compact, `^x` abort) dispatch through W3b's `src/ctl` routing. Picking the current model or level again changes nothing. A second SIGINT, SIGTERM or SIGHUP during quit restores the terminal and exits at once. Two identical signals that the kernel merges before glen's handler runs count as one, because standard POSIX signals are not queued, so press Ctrl-C again to force an exit.
- **W6** site: `site/`, static Astro in the Nodex-redesign look; a CSS-compiled terminal demo; morphin islands; palette from `src/render/palette.json`; CI job `site`.
- **W7** deploy + release workflow: §A18.
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
- [ ] Re-check extension agent_end original flags versus tail re-tag, prompt bracket unwinding, before_agent_start prompt joining, hasPendingMessages, loadMode/approval, factory rebinding and ctx.agent, managed timers, and -e/--no-extensions; re-run the Step 14 live smoke.
- [ ] Bump `TESTED_OMP_VERSION` only after all of these checks.
- **Open, owned by later waves:**
  - [x] W1: confirmed Linux abstract datagram bind semantics against `crates/pi-natives/src/file_lock/linux.rs:14-22` and the `.lock` suffix against `utils/src/file-lock.ts:31-33`. Tests cover both golden names and `<id>.lock` path suffixes.
  - [x] W2: lost detection records pid plus process start time, rejects zombies/reused pids, and gives null-pid starts a 10 s grace.
  - [x] W3 implementation: GLEN_MANAGED gates all registration; main TUI endpoints supply busy/idle and identity changes. Unit/integration suites cover the contract; this check does not attest real-omp runtime evidence.
  - [x] W3 real-omp Step 14: run on the host — see evidence `.claude/evidence/v010-wave-3-live/README.md`; Step 5 FAIL led to the O6 settlement/attribution amendment. Post-fix Step 5 must be rerun on the host before acceptance.
  - [x] W4: Codex shell timeout/output budget and Claude Code allowed-tools syntax verified; guarded skill and JSON contracts shipped. Real Step 11 acceptance remains separate.
  - [ ] Re-verify Claude Code skill frontmatter/allowed-tools and Codex skill paths, limits, shell timeout and sandbox variables on each Claude Code or Codex upgrade.

- [ ] W4 tech debt: verify Linux network-on CODEX_SANDBOX live delivery and EROFS behaviour under bubblewrap.

- [ ] W3b upgrade checks: re-check all six RPC controls and their wire payloads; Model not found / No API key prefixes; unchanged set_model append, thinking append-on-effective-change, configured thinking and resume defaults; compaction abort/resume and abort keeping queues; TUI setModel/setThinkingLevel/compact/abort wiring; ctx.models.list versus RPC filtering; serial queue including abort. Re-run the Step 12 scratch smoke outside the sandbox, verify persisted model/thinking on resume and unchanged global config, record evidence, and leave/list scratch sessions for the owner.

### §A18 Distribution
<!-- Level 3 for now: tests/conformance/repo_contracts.rs (AC-024) pins exactly 17 level-2 sections. Promote to ## when that test learns §A18. -->
- **Targets** (W7 plan D1, D2). Four, each built on a native GitHub runner with `cargo build --release --locked`:

| Target | Runner | Smoke test |
|---|---|---|
| `aarch64-apple-darwin` | `macos-15` | runs `--version`, `ls --all`, `show` |
| `x86_64-apple-darwin` | `macos-15`, cross-arch with Apple's toolchain | under Rosetta when present, else `lipo -archs` prints `x86_64` |
| `x86_64-unknown-linux-musl` | `ubuntu-24.04` | runs; `file` reports a static binary |
| `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` | runs; `file` reports a static binary |

  - Linux binaries are static musl, so they run on any glibc (Debian 12, Ubuntu 22.04, RHEL 9). This holds while the built graph has no C dependency; reassess musl before adding one. Verify with `cargo tree -e build -i cc --target all` producing no dependency tree: ratatui's unbuilt optional backends can put `cc` in `Cargo.lock` without building it for any target.
- **Assets** (D3). Versionless names, so "latest" needs no GitHub API: `glen-<target>.tar.gz`, `glen-<target>.tar.gz.sha256` (`<64 hex>  glen-<target>.tar.gz`) and `SHA256SUMS`. Each archive holds exactly `glen-<target>/glen` (0755) and `glen-<target>/LICENSE`. `scripts/package-release.sh <target> <binary> <out-dir>` builds both files. Checksums show integrity, not authenticity; binaries are not signed or notarised.
- **`install.sh`**, POSIX `sh`, served only from `https://raw.githubusercontent.com/nodera-studio/glen/main/install.sh` (D4):
  - flags `--version X.Y.Z` (an optional leading `v`), `--prefix DIR` (installs `DIR/bin/glen`; default `$HOME/.local/bin`), `--dry-run` (prints target, URLs and install path; no download, no write), `-h`/`--help`;
  - exit 0 ok, 1 runtime failure (unsupported platform, download, checksum, archive entry, unwritable directory), 2 usage;
  - target from `uname -s`/`uname -m`; a Rosetta shell on Apple silicon (`sysctl.proc_translated` = 1) gets the arm64 build;
  - downloads `<base>/latest/download/<asset>` or `<base>/download/v<ver>/<asset>`, where `<base>` is `GLEN_INSTALL_BASE_URL` (mirrors and tests) or `https://github.com/nodera-studio/glen/releases`;
  - verifies the SHA-256 and the archive's entry list before extracting, then swaps the binary in atomically. It never uses sudo.
- **Release workflow** (`.github/workflows/release.yml`, D5):
  - **dry run** on `workflow_dispatch` and on pull requests that change `release.yml`, `install.sh` or `scripts/package-release.sh`: build, smoke-test, package, install from a `file://` fake release on Linux and macOS, upload the archives as workflow artifacts. No tag, no release.
  - **publish** on a pushed `v*` tag: the preflight requires an annotated tag equal to `v` + the `Cargo.toml` version on a commit of `main`; then the same jobs, an exact 8-file asset check, `SHA256SUMS`, and a GitHub Release with generated notes (a `-` in the tag makes it a pre-release). A published release is never overwritten.
- **Site hosting** (D7, D8). One assets-only Cloudflare Worker named `glen` on workers.dev (`site/wrangler.jsonc`: no `main`, no bindings, no routes, `not_found_handling: "404-page"`). `site/public/_headers` sets `Cache-Control: public, max-age=31536000, immutable` on `/_astro/*` only; HTML keeps the platform default (`public, max-age=0, must-revalidate`), because a header set by two rules is comma-joined.
  - `npm --prefix site run deploy` runs `verify` (including the offline link check), `check-links.mjs --online`, then `wrangler@4.124.0 deploy` on the local login.
  - CI's `site-deploy` job runs `wrangler deploy --dry-run` on every PR and push. On a `main` push it also runs the online link check, and deploys the tested `dist/` only when the repo secret `CLOUDFLARE_API_TOKEN` exists; otherwise it prints a notice.
  - The Install section shows no installer URL or `/releases/latest` until `site.releaseAvailable` is true; W8 flips it after v0.1.0 is published, and the link check enforces both states.
- **Demo golden** (D6). `tests/site_demo.rs` runs the real binary on `tests/fixtures/demo` and compares its ANSI stdout with `site/src/data/demo-capture.json`; `npm --prefix site run demo:refresh` rewrites it. The site decodes the capture at build (`site/src/data/ansi.mjs`), so the demo cannot drift from glen. Only TTY-only stderr (spinner, progress, the `tail -f` diagnostic) is hand-written, and the site's unit tests check those strings against `src/`.
