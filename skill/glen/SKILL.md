---
name: glen
description: "Talk to omp (oh-my-pi) coding-agent sessions with the glen CLI: list them, read their transcripts, send a prompt, steer a running turn or queue a follow-up, wait for the answer, and read what omp sends back. Use when the user mentions omp, oh-my-pi, an omp session or TUI, or asks you to ask, tell, check on, hand work to, or read the output of an agent running in omp. Not for Claude Code or Codex sessions."
allowed-tools: Bash(glen *)
---

# glen: talk to omp sessions

Run glen as one plain command per call (`glen ls`, no pipes, no `jq`, no compound commands).
When stdout is not a terminal glen prints one compact JSON document; in a terminal add `--json`.
Every command has `--help`.

## The loop
1. Find: `glen ls` lists this project's sessions (`--cwd DIR` for another project, `--all` for every project,
   `--live` for open ones, `-n 50` for more rows; default 20). Rows have `handle` (7 hex characters), `title`,
   `first_message` and `state`: `idle`, `live` (open in omp), `busy` (working), `unknown`.
2. Read: `glen tail <handle> -n 20` for latest items (default 10), `glen show <handle>` for the active branch
   (`--tools full` and `--thinking` add detail).
3. Send: `glen send <handle> "message"` delivers and waits up to 90 s. `@latest` is the project's newest session;
   `glen send @new "message" --cwd DIR` starts a new one.
4. Answer: `data.assistant_text` is omp's final answer; `data.replies` holds its `glen_reply` messages.
   Exit 10 means still working: `glen wait <job>` using `data.job.handle` (starts with `j`).
   To be woken by each reply instead, see "Replies without polling".
5. Later: replies after the job ended and messages from the omp TUI: see "Replies without polling". <!-- glen:if inbox -->

## Exit codes
| exit | code | do this |
|---|---|---|
| 0 | — | done; read `data` |
| 1 | `internal`, `io` | report `error.message` |
| 2 | `usage`, `invalid_env` | fix the command or variable named in `error.message`; a missing argument's name follows the colon |
| 3 | `not_found` | follow `error.hint` (sessions: `glen ls --all`; jobs: `glen jobs`; models: `glen models <handle>`) |
| 4 | `ambiguous` | pick a candidate from `error.details.candidates` (a session handle or provider/id) |
| 5 | `omp_unavailable` | tell the user; `glen doctor` shows why |
| 6 | `no_route` | refused safely; see below |
| 7 | `job_failed`, `control_failed` | report `error.details` |
| 8 | `not_implemented` | this version lacks the command |
| 10 | running | `glen wait <job>`; the job keeps running. With `--for reply`, `data.woke` of `reply` means new replies; no `woke` means only the bound passed |
| 11 | forked | done; read `data`, then use `data.forked.to_handle` |
| 130 | interrupted | send/wait: `glen wait <job>`; tail -f: stream ended |

Exit 6: read `error.hint` and `error.details.reason`, tell the user, and do not retry in a loop or work around it.
- `sandboxed`: rerun this command outside the sandbox (ask for escalated permissions for `glen`);
  with `lease:"unknown"` (Linux) escalating does not help: follow `error.hint`.
  With `glen_home_writable:false`, set `GLEN_HOME` to a private directory you own, e.g. `"$(mktemp -d)/glen"`,
  or run outside the sandbox.
- Otherwise the session is open in omp and glen cannot deliver safely; follow `error.hint`.
- Usually fix with `glen install-extension` (writes omp config, so ask first), then ask the user to restart omp. <!-- glen:if install-extension -->

## Output
- Success: `{"v":1,"ok":true,"data":{…},"warnings":[…]}`. Failure: `{"v":1,"ok":false,"error":{"code","message","hint","details"}}`.
  `ok` is true for exits 0, 10, 11 and 130. Read `warnings` when present.
- Large output is `{"v":1,"ok":true,"spilled":{"path":"…","bytes":…,"format":"json"}}`: read that file.
  Prefer `glen tail <handle> -n 20` for long sessions.
- `glen tail <handle> -f` streams JSON lines until `{"v":1,"event":"end","ok":true}`; use only if your shell can stream.

## Sending well
- Busy sessions: default `--as auto` queues a follow-up after the running turn (`--follow-up` is explicit).
  `--steer` puts it into the active turn at the next step. Idle sessions: every mode starts a turn.
- Jobs keep running after glen stops waiting: `--wait 30s` shortens the wait, `--no-wait` returns immediately;
  `glen jobs --active` lists running jobs (default 20 rows).
- omp has `glen_reply` (`text`, optional `final`) in sessions glen reaches. Ask for it for progress or specific answers;
  replies arrive in `data.replies`.
- Every reply also lands in the inbox (`glen inbox --peek` lists it), with `job_id` naming its job. <!-- glen:if inbox -->
- Send quoted text: `glen send <handle> -- "-v is broken"`; `--` lets a message start with `-`.
- Never send the same message twice. After exit 10, exit 130 or a call your shell cut off, it is still queued or
  running: `glen jobs --active`, then `glen wait <job>`.
- Another send coming soon: `glen send <handle> "…" --warm 5m` keeps omp alive up to 5m so it skips startup. <!-- glen:if release -->
  Warm holds the session (the omp TUI cannot open it) until it ends or `glen release <handle>` (`glen release --all` for all). <!-- glen:if release -->

## Replies without polling
Never poll the inbox in a loop or re-run `glen wait` on a timer: start one receiver that returns when something arrives.
- `glen wait <job> --for reply` returns when omp calls `glen_reply` for that job. Exit 10 with `data.woke` of
  `reply`: read `data.replies`, then run `data.next` (it adds `--after <last reply id>`, so nothing repeats).
  Exit 10 without `woke`: only the bound passed; run `data.next` again. Exit 0, 6, 7 or 11: the job ended, and
  `data.replies` (or `error.details.replies`) holds every reply you have not seen.
- Claude Code: after `glen send <handle> "…" --no-wait`, start the receiver as a background Bash command
  (`run_in_background`) and end your turn; its completion wakes you.
- Claude Code subagent: nothing wakes you. Run the receiver in the foreground with `--timeout 90s` (Bash stops
  at 2 minutes), or return the job handle and the last reply id to the parent, which starts the background wait.
- Codex: no push. Run `glen wait <job> --for reply --timeout 25s` through `exec_command`; if the call yields
  first, poll that session until it ends. On exit 10 act on `data.replies` and repeat with `data.next`.
  Run `glen inbox --peek` once at the start of each turn for TUI messages and late replies. <!-- glen:if inbox -->
- Late and TUI replies (no job to wait on): `glen inbox --wait` waits up to 90 s (`--timeout DUR`, at most 24 h): <!-- glen:if inbox -->
  exit 0 with unread messages, exit 10 with none. Replies seen through `glen wait` stay unread there; skip ids you have. <!-- glen:if inbox -->
  `glen inbox --wait` with `--peek` returns existing unread messages at once on every call: a one-shot read, not a receiver. <!-- glen:if inbox -->

## Shell timeouts and sandboxes
- `glen send` and `glen wait` default to 90 s; give them a shell timeout of at least 100 s.
  Codex's classic shell kills after 10 s unless you set a timeout; exec_command yields after 10 s and keeps running.
  Use `glen send <handle> "…" --wait 5s`, then `glen wait <job> --timeout 5s` while exit is 10.
  After a cut-off send, use `glen jobs --active`.
- Session controls (compact, model, thinking, abort) also default to 90 s: give them a shell timeout of at least 100 s or pass a shorter `--timeout`. SIGTERM and SIGHUP stop the headless omp they started. <!-- glen:if compact model thinking abort -->
- In Codex's sandbox reads work. Spills need writable `GLEN_HOME`; otherwise exit 6 `sandboxed` with
  `glen_home_writable:false`: set `GLEN_HOME` to a private directory you own, e.g. `"$(mktemp -d)/glen"`.
  Prefer `glen tail <handle> -n 20`.
  Sending and session controls must reach omp outside the sandbox; exit 6 `sandboxed` explains the restriction.
  Only non-empty `CODEX_SANDBOX` marks a sandbox. `CODEX_SANDBOX_NETWORK_DISABLED=1` alone allows sends;
  a real permission failure returns exit 6 with the hint: rerun with escalated permissions.
- On Linux, under either variable (escalated too), `ls` shows existing sessions `unknown` and resuming one over RPC
  refuses exit 6 `sandboxed` with `lease:"unknown"`. Escalating cannot fix that: use the live route (the session
  open in the omp TUI with glen's extension), a new session (`@new`), or ask the user to run glen outside Codex.
- Network-enabled sandboxes support live sessions only, with writable `GLEN_HOME`. <!-- glen:if install-extension -->
- Read sandboxed replies with `glen inbox --peek` to avoid writing a read marker; if the marker cannot be written, `glen inbox --wait` exits 6 and leaves them unread. <!-- glen:if inbox -->

<!-- glen:if abort stop models model thinking compact -->
## Session controls
- `glen abort <handle>` stops the running turn (`glen stop <handle>` is the same); idle abort starts nothing.
- `glen models` lists the profile models without resuming a session. `glen models <handle>` reports its selection;
  `--provider P` filters providers. `glen model <handle> <provider/id>` switches the model;
  `glen thinking <handle> high` accepts off, minimal, low, medium, high, xhigh, max when supported by that model.
  `glen compact <handle>` needs an idle session; abort the run first or wait.
- Controls and models take `--timeout DUR` (default 90s). Changes have session scope; auto/inherit are unavailable.
  Models may fall back to disk selection with session_unreachable or extension_outdated warnings.
  Exit 6 extension_outdated: run `glen install-extension`, then restart that omp session.
  Abort keeps queued omp messages; queued_jobs names glen jobs that start next.
  A live timeout may still finish; a job timeout has unknown outcome: check the job before retrying.
  Exit 7 `outcome_unknown`: the change may have been applied; check `glen models <handle>` before retrying.
- These change the user's session: run only when asked.
<!-- glen:end -->

## Rules
- Never write, move or delete omp's files (`~/.omp`). glen reads and sends through omp.
- Never work around a refusal: don't start omp on a session yourself or copy it. Two writers fork a session.
- Don't send into a session someone is using unless asked; on a busy session prefer default follow-up.
- Setup: `glen doctor`. Update: `glen install-skill`. Completions for people: `glen completions zsh`.
- MCP hosts can register `glen mcp` and call glen as tools (`glen_ls`, `glen_send`, `glen_wait`, …) with the same envelopes (`isError` is `ok:false`); never run it from Bash. <!-- glen:if mcp -->
  In a tool call `send` and `wait` still wait up to 90 s: pass a shorter `wait` or `timeout` if the host cuts calls sooner. <!-- glen:if mcp -->
<!-- glen:if watch -->
- For people in an interactive terminal: `glen watch <handle>` (exits 2 without a terminal); agents should not run it and use `glen tail <handle> -f` for a transcript and the receivers under Replies without polling for replies.
<!-- glen:end -->
