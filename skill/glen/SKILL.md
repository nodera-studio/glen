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
5. Later: `glen inbox` shows late replies and messages from the omp TUI; `--peek` leaves them unread. <!-- glen:if inbox -->

## Exit codes
| exit | code | do this |
|---|---|---|
| 0 | — | done; read `data` |
| 1 | `internal`, `io` | report `error.message` |
| 2 | `usage`, `invalid_env` | fix the command or variable named in `error.message` |
| 3 | `not_found` | `glen ls --all` and use a listed handle |
| 4 | `ambiguous` | use a handle from `error.details.candidates` |
| 5 | `omp_unavailable` | tell the user; `glen doctor` shows why |
| 6 | `no_route` | refused safely; see below |
| 7 | `job_failed`, `control_failed` | report `error.details` |
| 8 | `not_implemented` | this version lacks the command |
| 10 | running | `glen wait <job>`; the job keeps running |
| 11 | forked | done; read `data`, then use `data.forked.to_handle` |
| 130 | interrupted | send/wait: `glen wait <job>`; tail -f: stream ended |

Exit 6: read `error.hint` and `error.details.reason`, tell the user, and do not retry in a loop or work around it.
- `sandboxed`: rerun this command outside the sandbox (ask for escalated permissions for `glen`).
  With `glen_home_writable:false`, set `GLEN_HOME` under `$TMPDIR` or run outside the sandbox.
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
- Replies also land in `glen inbox`, with `job_id` naming their job. <!-- glen:if inbox -->
- Send quoted text: `glen send <handle> -- "-v is broken"`; `--` lets a message start with `-`.
- Never send the same message twice. After exit 10, exit 130 or a call your shell cut off, it is still queued or
  running: `glen jobs --active`, then `glen wait <job>`.

## Shell timeouts and sandboxes
- `glen send` and `glen wait` default to 90 s; give them a shell timeout of at least 100 s.
  Codex's classic shell kills after 10 s unless you set a timeout; exec_command yields after 10 s and keeps running.
  Use `glen send <handle> "…" --wait 5s`, then `glen wait <job> --timeout 5s` while exit is 10.
  After a cut-off send, use `glen jobs --active`.
- In Codex's sandbox reads work. Spills need writable `GLEN_HOME`; otherwise exit 6 `sandboxed` with
  `glen_home_writable:false`: set `GLEN_HOME` under `$TMPDIR`. Prefer `glen tail <handle> -n 20`.
  Sending and session controls must reach omp outside the sandbox; exit 6 `sandboxed` explains the restriction.
  Only non-empty `CODEX_SANDBOX` marks a sandbox. `CODEX_SANDBOX_NETWORK_DISABLED=1` alone allows sends
  (on Linux, under either variable and even escalated, existing sessions only through the live route;
  resuming over RPC refuses exit 6 `sandboxed`, `lease:"unknown"`, and `ls` shows those sessions `unknown`);
  a real permission failure returns exit 6 with the hint. Rerun with escalated permissions if refused.
- Network-enabled sandboxes support live sessions only, with writable `GLEN_HOME`. <!-- glen:if install-extension -->
- Read sandboxed replies with `glen inbox --peek` to avoid writing a read marker. <!-- glen:if inbox -->

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
- These change the user's session: run only when asked.
<!-- glen:end -->

## Rules
- Never write, move or delete omp's files (`~/.omp`). glen reads and sends through omp.
- Never work around a refusal: don't start omp on a session yourself or copy it. Two writers fork a session.
- Don't send into a session someone is using unless asked; on a busy session prefer default follow-up.
- Setup: `glen doctor`. Update: `glen install-skill`. Completions for people: `glen completions zsh`.
<!-- glen:if watch -->
- For people in an interactive terminal: `glen watch <handle>` (exits 2 without a terminal); agents should not run it and use `glen tail <handle> -f` and `glen inbox -f` instead.
<!-- glen:end -->
