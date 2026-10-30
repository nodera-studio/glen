# glen JSON contract (v1)

This is the normative per-command contract. `docs/design.md` §A10 keeps the shared rules.
Examples use fixture sessions and fake RPC peers, with representative values. Consumers must use fields,
not the prose of diagnostic messages. `tests/json_contract.rs` compares these shapes with real envelopes.

## Modes and streams

A TTY gets human output; other stdout gets compact JSON. `--json` and `--human` override this choice.
Explicit JSON on a TTY is pretty printed. JSON stdout contains the result and stderr is empty.
`--help` and `--version` always print text. `completions` defaults to raw shell script, including on a pipe;
add `--json` for its envelope. Its errors without `--json` go to stderr as human text.

Success is `{"v":1,"ok":true,"data":{…}}`. Errors are `{"v":1,"ok":false,"error":{"code":"…","message":"…"}}`.
Error `hint` and `details` are optional. Optional top-level `warnings` is an array of `{code,message}`.
`ok:true` applies to exits 0, 10, 11 and 130. A spilled success replaces `data` with
`spilled:{path,bytes,format}`: open that absolute path to read the full envelope (0600, exact byte count).
`GLEN_MAX_OUTPUT` defaults to 24000 bytes on a non-TTY; 0 disables spilling. Streams never spill.

`tail -f` emits NDJSON: one compact object per line with `v:1` and an `event` discriminator.
Events are `session`, `item`, `continued`, `rewritten`, `branch_switch`, `warning`, then exactly one terminal
`{"v":1,"event":"end","ok":true}` or error envelope, last. SIGINT ends the stream with exit 130.
A second SIGINT terminates immediately and may prevent a terminal line.
`inbox -f` is always NDJSON, even with `--human`: `message` events, then the same terminal line.

## Exit codes

| exit | ok | error.code / status | meaning |
|---|---|---|---|
| 0 | true | done | result is complete |
| 1 | false | `internal`, `io` | implementation or filesystem failure |
| 2 | false | `usage`, `invalid_env` | arguments or environment need correction |
| 3 | false | `not_found` | session or job not found |
| 4 | false | `ambiguous` | use a candidate handle |
| 5 | false | `omp_unavailable` | omp missing, broken or timed out |
| 6 | false | `no_route` | refusal: live owner, unknown lease, or sandbox |
| 7 | false | `job_failed` | failed, aborted or lost job |
| 8 | false | `not_implemented` | hidden unfinished command |
| 10 | true | running | job continues; use wait |
| 11 | true | forked | use forked.to_handle |
| 130 | true | interrupted | foreground observer stopped; job continues |

## Field conventions and stability

`v:1` is append-only: ignore unknown fields and warning codes. Keys are snake_case; timestamps are RFC 3339
UTC; durations are integer `*_ms`; paths are absolute. Defined nullable data fields remain present as `null`.
Optional error diagnostics and warnings may be omitted. Error codes, reason values and field names are stable;
`message`, `hint` and `detail` prose is not. `show --raw` returns omp's own records, outside this schema.

Session objects have `handle,id,path,bucket,cwd,title,first_message,created_at,updated_at,age_ms,size_bytes,model,
last_turn,state,state_source,state_detail,continuation_of,parent_session`. State is `idle|live|busy|unknown`.
Handles are unique catalog suffixes, normally 7 hex characters. Nullable model and continuation fields stay null.

Transcript items share `kind,id,timestamp`. Kind-specific fields are: user `text,images`; assistant
`text,model,stop_reason,error,has_thinking,thinking_words` (plus thinking when requested and tool_calls when nonempty);
tool_result `tool_call_id,tool_name,is_error,text,chars,truncated,images`; bash/python `command|code,exit_code,cancelled,
output,chars,truncated,images`; developer `text`; custom `custom_type,text`; file_mention `files:[{path,line_count}]`;
compaction `tokens_before,tokens_after,summary`; reset has no extra fields; branch_summary `summary`;
model_change `model`; thinking_level `level`. Consumers dispatch by kind rather than assuming homogeneous arrays.
Unknown omp records are skipped by the rendered reader and preserved by raw output.

Job views share `id,handle,status,route,deliver_as,delivered_as,session_id,session_handle,actual_session_id,
created_at,updated_at,elapsed_ms`. Routes are `rpc|new|live`.
Replies carry `text,final` and context/sequence metadata; read their field names from the examples below.

## `glen ls`

List this project (or `--cwd DIR`, `--all`) with filters applied before the limit.

| Field | Meaning |
|---|---|
| `scope` | all, cwd and bucket; cwd/bucket are null with --all |
| `sessions_dir` | absolute resolved source |
| `total, total_all` | filtered and catalog counts |
| `sessions` | session objects in newest-first order |

<!-- contract: ls -->
```json
{
  "data": {
    "scope": {
      "all": false,
      "bucket": "-work-alpha",
      "cwd": "/tmp/glen-example/home/work/alpha"
    },
    "sessions": [
      {
        "age_ms": 60967,
        "bucket": "-work-alpha",
        "continuation_of": null,
        "created_at": "2026-10-05T16:20:00Z",
        "cwd": "/home/fixture/work/alpha",
        "first_message": "Placeholder request.",
        "handle": "b8e4a07",
        "id": "01a10cdd-6b80-7a29-9c5e-6d1f3b8e4a07",
        "last_turn": "complete",
        "model": "anthropic/claude-sonnet-4-5",
        "parent_session": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
        "path": "/tmp/glen-example/agent/sessions/-work-alpha/2026-10-05T16-20-00-000Z_01a10cdd-6b80-7a29-9c5e-6d1f3b8e4a07.jsonl",
        "size_bytes": 1259,
        "state": "idle",
        "state_detail": null,
        "state_source": "disk",
        "title": "Fork a placeholder session",
        "updated_at": "2026-10-30T07:52:10.499242Z"
      }
    ],
    "sessions_dir": "/tmp/glen-example/agent/sessions",
    "total": 4,
    "total_all": 7
  },
  "ok": true,
  "v": 1
}
```

## `glen show`

Read the active branch. `--tools` and `--thinking` control detail; `--raw` returns original entries.

| Field | Meaning |
|---|---|
| `session` | resolved session object |
| `continued_from` | continuation source or null |
| `leaf_id` | active branch leaf or null |
| `skipped_lines` | malformed lines skipped |
| `items / entries` | rendered items, or raw omp entries |

<!-- contract: show -->
```json
{
  "data": {
    "continued_from": [],
    "items": [
      {
        "id": "10000001",
        "kind": "thinking_level",
        "level": "high",
        "timestamp": "2026-10-01T09:00:01Z"
      }
    ],
    "leaf_id": "10000009",
    "session": {
      "age_ms": 360976,
      "bucket": "-work-alpha",
      "continuation_of": null,
      "created_at": "2026-10-01T09:00:00Z",
      "cwd": "/home/fixture/work/alpha",
      "first_message": "Why does parser_test flake?",
      "handle": "e27c0de",
      "id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
      "last_turn": "complete",
      "model": "anthropic/claude-sonnet-4-5",
      "parent_session": null,
      "path": "/tmp/glen-example/agent/sessions/-work-alpha/2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.jsonl",
      "size_bytes": 3438,
      "state": "idle",
      "state_detail": null,
      "state_source": "disk",
      "title": "Fix the flaky parser test",
      "updated_at": "2026-10-30T07:47:10.499242Z"
    },
    "skipped_lines": 0
  },
  "ok": true,
  "v": 1
}
```

<!-- contract: show.raw -->
```json
{
  "data": {
    "continued_from": [],
    "entries": [
      {
        "configured": "high",
        "id": "10000001",
        "parentId": null,
        "runtime": {
          "version": "18.14.0"
        },
        "thinkingLevel": "high",
        "timestamp": "2026-10-01T09:00:01.000Z",
        "type": "thinking_level_change"
      }
    ],
    "leaf_id": "10000009",
    "session": {
      "age_ms": 360984,
      "bucket": "-work-alpha",
      "continuation_of": null,
      "created_at": "2026-10-01T09:00:00Z",
      "cwd": "/home/fixture/work/alpha",
      "first_message": "Why does parser_test flake?",
      "handle": "e27c0de",
      "id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
      "last_turn": "complete",
      "model": "anthropic/claude-sonnet-4-5",
      "parent_session": null,
      "path": "/tmp/glen-example/agent/sessions/-work-alpha/2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.jsonl",
      "size_bytes": 3438,
      "state": "idle",
      "state_detail": null,
      "state_source": "disk",
      "title": "Fix the flaky parser test",
      "updated_at": "2026-10-30T07:47:10.499242Z"
    },
    "skipped_lines": 0
  },
  "ok": true,
  "v": 1
}
```

## `glen tail`

Read latest items (default 10). `-n 0 -f` streams only new events.

| Field | Meaning |
|---|---|
| `session, continued_from, leaf_id, skipped_lines` | same as show in document mode |
| `items` | latest requested transcript items |
| `event` | NDJSON discriminator; session/item then exactly one end |

<!-- contract: tail -->
```json
{
  "data": {
    "continued_from": [],
    "items": [
      {
        "error": null,
        "has_thinking": false,
        "id": "10000007",
        "kind": "assistant",
        "model": "anthropic/claude-sonnet-4-5",
        "stop_reason": "stop",
        "text": "The test depends on map order. Fixed.",
        "thinking_words": 0,
        "timestamp": "2026-10-01T09:00:07Z"
      }
    ],
    "leaf_id": "10000009",
    "session": {
      "age_ms": 360992,
      "bucket": "-work-alpha",
      "continuation_of": null,
      "created_at": "2026-10-01T09:00:00Z",
      "cwd": "/home/fixture/work/alpha",
      "first_message": "Why does parser_test flake?",
      "handle": "e27c0de",
      "id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
      "last_turn": "complete",
      "model": "anthropic/claude-sonnet-4-5",
      "parent_session": null,
      "path": "/tmp/glen-example/agent/sessions/-work-alpha/2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.jsonl",
      "size_bytes": 3438,
      "state": "idle",
      "state_detail": null,
      "state_source": "disk",
      "title": "Fix the flaky parser test",
      "updated_at": "2026-10-30T07:47:10.499242Z"
    },
    "skipped_lines": 0
  },
  "ok": true,
  "v": 1
}
```

<!-- contract: tail.follow ndjson -->
```json
{"continued_from":[],"event":"session","session":{"age_ms":361074,"bucket":"-work-alpha","continuation_of":null,"created_at":"2026-10-01T09:00:00Z","cwd":"/home/fixture/work/alpha","first_message":"Why does parser_test flake?","handle":"e27c0de","id":"01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de","last_turn":"complete","model":"anthropic/claude-sonnet-4-5","parent_session":null,"path":"/tmp/glen-example/agent/sessions/-work-alpha/2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.jsonl","size_bytes":3438,"state":"idle","state_detail":null,"state_source":"disk","title":"Fix the flaky parser test","updated_at":"2026-10-30T07:47:10.499242Z"},"skipped_lines":0,"v":1}
{"event":"item","item":{"error":null,"has_thinking":false,"id":"10000007","kind":"assistant","model":"anthropic/claude-sonnet-4-5","stop_reason":"stop","text":"The test depends on map order. Fixed.","thinking_words":0,"timestamp":"2026-10-01T09:00:07Z"},"v":1}
{"event":"end","ok":true,"v":1}
```

## `glen send`

Send once. Exits 10 and 130 leave the job running; never resend to poll. Use `wait`.

| Field | Meaning |
|---|---|
| `status` | completed or running |
| `job` | job view including route |
| `session` | actual id, handle, path (null until known for new jobs) |
| `assistant_text, replies` | answer and glen_reply records |
| `command_output, stop_reason` | local command result or model stop reason, nullable |
| `session_settled, delivered_as` | persistence acknowledged; prompt/steer/follow-up or null |
| `forked` | null or {from,to,to_handle} |
| `next, interrupted` | running-only wait command and observer interruption flag |

<!-- contract: send.completed -->
```json
{
  "data": {
    "assistant_text": "pong: hi",
    "command_output": [],
    "delivered_as": "prompt",
    "forked": null,
    "job": {
      "actual_session_id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
      "created_at": "2026-10-30T07:53:11.792831Z",
      "deliver_as": "auto",
      "delivered_as": "prompt",
      "elapsed_ms": 614,
      "handle": "j6977273",
      "id": "01a18bcc-6970-7685-8f03-0ecc16977273",
      "route": "rpc",
      "session_handle": "e27c0de",
      "session_id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
      "status": "completed",
      "updated_at": "2026-10-30T07:53:12.406814Z"
    },
    "replies": [
      {
        "at": "2026-10-30T07:53:12.332571Z",
        "final": true,
        "id": "01a18bcc-6b8c-74bc-81aa-d8b5134e7534",
        "job_id": "01a18bcc-6970-7685-8f03-0ecc16977273",
        "request_id": null,
        "session_id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
        "text": "reply: hi",
        "tool_call_id": "call_1",
        "v": 1
      }
    ],
    "session": {
      "handle": "e27c0de",
      "id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
      "path": "/tmp/glen-example/agent/sessions/-work-alpha/2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.jsonl"
    },
    "session_settled": true,
    "status": "completed",
    "stop_reason": "stop"
  },
  "ok": true,
  "v": 1
}
```

<!-- contract: send.running -->
```json
{
  "data": {
    "interrupted": false,
    "job": {
      "actual_session_id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
      "created_at": "2026-10-30T07:53:13.047024Z",
      "deliver_as": "auto",
      "delivered_as": "prompt",
      "elapsed_ms": 133,
      "handle": "j3f52ae7",
      "id": "01a18bcc-6e57-7495-aa65-cf0d73f52ae7",
      "route": "rpc",
      "session_handle": "e27c0de",
      "session_id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
      "status": "accepted",
      "updated_at": "2026-10-30T07:53:13.108631Z"
    },
    "next": "glen wait j3f52ae7",
    "session": {
      "handle": "e27c0de",
      "id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
      "path": "/tmp/glen-example/agent/sessions/-work-alpha/2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.jsonl"
    },
    "status": "running"
  },
  "ok": true,
  "v": 1
}
```

<!-- contract: send.forked -->
```json
{
  "data": {
    "assistant_text": "pong: hi",
    "command_output": [],
    "delivered_as": "prompt",
    "forked": {
      "from": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
      "to": "01a0f6b1-2680-7a3c-8b21-4a1f9e27f0f0",
      "to_handle": "e27f0f0"
    },
    "job": {
      "actual_session_id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27f0f0",
      "created_at": "2026-10-30T07:53:12.49555Z",
      "deliver_as": "auto",
      "delivered_as": "prompt",
      "elapsed_ms": 197,
      "handle": "j1ddc744",
      "id": "01a18bcc-6c2f-733f-98ed-59c791ddc744",
      "route": "rpc",
      "session_handle": "e27c0de",
      "session_id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
      "status": "completed",
      "updated_at": "2026-10-30T07:53:12.692935Z"
    },
    "replies": [
      {
        "at": "2026-10-30T07:53:12.626788Z",
        "final": true,
        "id": "01a18bcc-6cb2-70ed-876b-995d5e8bca88",
        "job_id": "01a18bcc-6c2f-733f-98ed-59c791ddc744",
        "request_id": null,
        "session_id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
        "text": "reply: hi",
        "tool_call_id": "call_1",
        "v": 1
      }
    ],
    "session": {
      "handle": "e27c0de",
      "id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
      "path": "/tmp/glen-example/agent/sessions/-work-alpha/2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.jsonl"
    },
    "session_settled": true,
    "status": "completed",
    "stop_reason": "stop"
  },
  "ok": true,
  "v": 1
}
```

## `glen wait`

Observe the same job; completed output has the send shape. A bounded wait can return running again.

| Field | Meaning |
|---|---|
| `data` | same running or completed data as send |

<!-- contract: wait -->
```json
{
  "data": {
    "assistant_text": "pong: hi",
    "command_output": [],
    "delivered_as": "prompt",
    "forked": null,
    "job": {
      "actual_session_id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
      "created_at": "2026-10-30T07:53:13.047024Z",
      "deliver_as": "auto",
      "delivered_as": "prompt",
      "elapsed_ms": 841,
      "handle": "j3f52ae7",
      "id": "01a18bcc-6e57-7495-aa65-cf0d73f52ae7",
      "route": "rpc",
      "session_handle": "e27c0de",
      "session_id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
      "status": "completed",
      "updated_at": "2026-10-30T07:53:13.888636Z"
    },
    "replies": [
      {
        "at": "2026-10-30T07:53:13.815191Z",
        "final": true,
        "id": "01a18bcc-7157-7342-8797-e42243599cb2",
        "job_id": "01a18bcc-6e57-7495-aa65-cf0d73f52ae7",
        "request_id": null,
        "session_id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
        "text": "reply: hi",
        "tool_call_id": "call_1",
        "v": 1
      }
    ],
    "session": {
      "handle": "e27c0de",
      "id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
      "path": "/tmp/glen-example/agent/sessions/-work-alpha/2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.jsonl"
    },
    "session_settled": true,
    "status": "completed",
    "stop_reason": "stop"
  },
  "ok": true,
  "v": 1
}
```

## `glen jobs`

List jobs, newest first. Absent glen home gives an empty list without creating it.

| Field | Meaning |
|---|---|
| `total` | after filters, before limit |
| `jobs` | job views plus text_preview, error and forked; -n 0 means all |

<!-- contract: jobs -->
```json
{
  "data": {
    "jobs": [{
      "id": "01a18bcc-6d3e-75f2-b9cf-6ffc7656f7d4",
      "handle": "j656f7d4",
      "status": "completed",
      "route": "rpc",
      "deliver_as": "auto",
      "delivered_as": "prompt",
      "session_id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
      "session_handle": "e27c0de",
      "actual_session_id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
      "created_at": "2026-10-30T12:00:00Z",
      "updated_at": "2026-10-30T12:00:01Z",
      "elapsed_ms": 1000,
      "text_preview": "hi",
      "error": null,
      "forked": null
    }],
    "total": 1
  },
  "ok": true,
  "v": 1
}
```

## `glen inbox`

Replies omp sent with `glen_reply`, oldest first. Reading marks them read unless `--peek`; `--all` includes read ones.
Missing glen home or inbox gives an empty list without creating it. `-f` streams NDJSON `message` events.

| Field | Meaning |
|---|---|
| `total` | messages returned |
| `marked_read` | true when this call advanced the read cursors |
| `skipped_lines` | malformed inbox lines skipped |
| `messages` | reply records plus session_handle and job_handle (null without a job) |

<!-- contract: inbox -->
```json
{
  "data": {
    "marked_read": false,
    "messages": [
      {
        "at": "2026-10-30T07:53:12.332571Z",
        "final": true,
        "id": "01a18bcc-6b8c-74bc-81aa-d8b5134e7534",
        "job_handle": "j6977273",
        "job_id": "01a18bcc-6970-7685-8f03-0ecc16977273",
        "request_id": null,
        "session_handle": "e27c0de",
        "session_id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
        "text": "reply: hi",
        "tool_call_id": "call_1",
        "v": 1
      }
    ],
    "skipped_lines": 0,
    "total": 1
  },
  "ok": true,
  "v": 1
}
```

<!-- contract: inbox.follow ndjson -->
```json
{"event":"message","message":{"at":"2026-10-30T07:53:12.332571Z","final":true,"id":"01a18bcc-6b8c-74bc-81aa-d8b5134e7534","job_handle":"j6977273","job_id":"01a18bcc-6970-7685-8f03-0ecc16977273","request_id":null,"session_handle":"e27c0de","session_id":"01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de","text":"reply: hi","tool_call_id":"call_1","v":1},"v":1}
{"event":"end","ok":true,"v":1}
```

## `glen doctor`

Read-only diagnostic. W4 rows are advisory and cannot turn success into failure.

| Field | Meaning |
|---|---|
| `glen` | version, home, home_exists |
| `omp` | bin, version, tested_version |
| `paths` | resolved omp paths and their sources |
| `extension` | path, state, version, current of the installed omp extension |
| `checks` | ordered {name,status,value,detail}, status ok/warn/fail/info |
| `skills` | claude/codex: path, state, version, current, project |
| `skills.*.project` | null or {path,state,version} |

<!-- contract: doctor -->
```json
{
  "data": {
    "checks": [
      {
        "detail": "",
        "name": "omp_binary",
        "status": "ok",
        "value": "/work/glen/tests/fixtures/bin/fake-omp"
      }
    ],
    "extension": {
      "current": false,
      "path": "/tmp/glen-example/agent/extensions/glen/index.ts",
      "state": "not_installed",
      "version": null
    },
    "glen": {
      "home": "/tmp/glen-example/glen",
      "home_exists": false,
      "version": "0.1.0"
    },
    "omp": {
      "bin": "/work/glen/tests/fixtures/bin/fake-omp",
      "tested_version": "18.14.0",
      "version": "18.14.0"
    },
    "paths": {
      "agent_dir": "/tmp/glen-example/agent",
      "agent_dir_source": "env",
      "base_config_root": "/tmp/glen-example/home/.omp",
      "config_root": "/tmp/glen-example/home/.omp",
      "extensions_dir": "/tmp/glen-example/agent/extensions",
      "profile": null,
      "session_owners_dir": "/tmp/glen-example/home/.omp/run/session-owners",
      "sessions_dir": "/tmp/glen-example/agent/sessions",
      "sessions_dir_source": "agent_dir"
    },
    "skills": {
      "claude": {
        "current": false,
        "path": "/tmp/glen-example/home/.claude/skills/glen/SKILL.md",
        "project": null,
        "state": "not_installed",
        "version": null
      },
      "codex": {
        "current": false,
        "path": "/tmp/glen-example/home/.agents/skills/glen/SKILL.md",
        "project": null,
        "state": "not_installed",
        "version": null
      }
    }
  },
  "ok": true,
  "v": 1
}
```

Check order: omp_binary, omp_version, profile, agent_dir, sessions_dir, glen_home, extension,
live_endpoints, omp_env_files, skill_claude, skill_codex, sandbox. `extension.state` is
current/not_installed/outdated/newer/different_home/modified/foreign; `live_endpoints` reports live and stale counts.
The `omp_env_files` check value is an array of absolute paths (empty when none need attention).

## `glen completions`

Raw script by default; JSON only when explicitly requested.

| Field | Meaning |
|---|---|
| `shell` | requested shell name |
| `script` | complete script string |

<!-- contract: completions -->
```json
{
  "data": {
    "script": "#compdef glen\n# shell completion script…\n",
    "shell": "zsh"
  },
  "ok": true,
  "v": 1
}
```

## `glen install-extension`

Install the bundled omp extension into the effective agent dir and profile. Dry runs make no changes.

| Field | Meaning |
|---|---|
| `action` | installed/unchanged/upgraded/reinstalled/replaced/removed/not_installed/skipped |
| `path, version, previous_version` | extension file, this build's version, marker version before the run or null |
| `glen_home` | GLEN_HOME baked into the file |
| `profile, agent_dir, agent_dir_source` | resolved omp profile (null for default) and agent dir |
| `dry_run, restart_required` | booleans; restart running omp sessions when the file changed |

<!-- contract: install-extension -->
```json
{
  "data": {
    "action": "installed",
    "agent_dir": "/tmp/glen-example/agent",
    "agent_dir_source": "env",
    "dry_run": true,
    "glen_home": "/tmp/glen-example/glen",
    "path": "/tmp/glen-example/agent/extensions/glen/index.ts",
    "previous_version": null,
    "profile": null,
    "restart_required": false,
    "version": "0.1.0"
  },
  "ok": true,
  "v": 1
}
```

## `glen install-skill`

Install for both agents unless selected; --project means cwd. Dry runs make no changes.

| Field | Meaning |
|---|---|
| `version, dry_run, scope` | binary version, boolean, user/project |
| `targets` | Claude then Codex: agent,path,action,state,installed_version |
| `targets[].state` | before-run not_installed/current/outdated/newer/modified/foreign |
| `targets[].action` | installed/unchanged/upgraded/skipped/reinstalled/removed/not_installed |
| `targets[].installed_version` | marker version before run, or null |

<!-- contract: install-skill -->
```json
{
  "data": {
    "dry_run": true,
    "scope": "project",
    "targets": [
      {
        "action": "installed",
        "agent": "claude",
        "installed_version": null,
        "path": "/tmp/glen-example/home/work/alpha/.claude/skills/glen/SKILL.md",
        "state": "not_installed"
      }
    ],
    "version": "0.1.0"
  },
  "ok": true,
  "v": 1
}
```

## `glen help`

Help and version are text in every output mode, with no JSON envelope.

## Error details

Usage errors may include clap diagnostics in details. Not-found errors supply a hint. Ambiguous errors include
`details.candidates`. Job failure has `details:{status,job,error,forked}`. A held lease has a hint without details.
Sandbox refusal adds `details:{reason:"sandboxed",sandbox,network}`; a permission-denied glen-home write adds
`glen_home_writable:false` and a hint naming `$TMPDIR` and writable_roots. Doctor failures embed data in
`details.report`. Install-skill filesystem failures include `details.done` (targets processed before failure).

<!-- contract: error.usage -->
```json
{
  "error": {
    "code": "usage",
    "details": {
      "text": "error: unexpected argument '--bogus' found\n\nUsage: glen ls [OPTIONS]\n\nFor more information, try '--help'.\n"
    },
    "message": "unexpected argument '--bogus' found"
  },
  "ok": false,
  "v": 1
}
```

<!-- contract: error.not_found -->
```json
{
  "error": {
    "code": "not_found",
    "hint": "run `glen ls --all`",
    "message": "no omp session matches `fffffff`"
  },
  "ok": false,
  "v": 1
}
```

<!-- contract: error.ambiguous -->
```json
{
  "error": {
    "code": "ambiguous",
    "details": {
      "candidates": [
        {
          "cwd": "/home/fixture/work/alpha",
          "handle": "e27c0de",
          "id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
          "path": "/tmp/glen-example/agent/sessions/-work-alpha/2026-10-01T09-00-00-000Z_01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de.jsonl",
          "title": "Fix the flaky parser test",
          "updated_at": "2026-10-30T07:47:10.499242Z"
        }
      ]
    },
    "hint": "use a longer handle or the file path",
    "message": "`c0de` matches 2 sessions"
  },
  "ok": false,
  "v": 1
}
```

<!-- contract: error.job_failed -->
```json
{
  "error": {
    "code": "job_failed",
    "details": {
      "error": {
        "code": "omp_error",
        "details": {
          "retryable": true
        },
        "message": "fake provider error"
      },
      "forked": null,
      "job": {
        "actual_session_id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
        "created_at": "2026-10-30T07:53:12.766529Z",
        "deliver_as": "auto",
        "delivered_as": "prompt",
        "elapsed_ms": 135,
        "handle": "j656f7d4",
        "id": "01a18bcc-6d3e-75f2-b9cf-6ffc7656f7d4",
        "route": "rpc",
        "session_handle": "e27c0de",
        "session_id": "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
        "status": "failed",
        "updated_at": "2026-10-30T07:53:12.901903Z"
      },
      "status": "failed"
    },
    "hint": "see /tmp/glen-example/glen/jobs/01a18bcc-6d3e-75f2-b9cf-6ffc7656f7d4/worker.log",
    "message": "job j656f7d4 failed: fake provider error"
  },
  "ok": false,
  "v": 1
}
```

<!-- contract: error.no_route -->
```json
{
  "error": {
    "code": "no_route",
    "hint": "run `glen install-extension`, then restart that omp session",
    "message": "session e27c0de is open in another omp process"
  },
  "ok": false,
  "v": 1
}
```

<!-- contract: error.no_route.sandboxed -->
```json
{
  "error": {
    "code": "no_route",
    "details": {
      "network": false,
      "reason": "sandboxed",
      "sandbox": "seatbelt"
    },
    "hint": "run this glen command outside the sandbox (request escalated permissions and approve the `glen` prefix), or enable network for the sandbox and use a session open in omp with glen's extension",
    "message": "`glen send` cannot reach omp from inside Codex's sandbox"
  },
  "ok": false,
  "v": 1
}
```

<!-- contract: error.omp_unavailable -->
```json
{
  "error": {
    "code": "omp_unavailable",
    "details": {
      "report": {
        "checks": [
          {
            "detail": "omp not found on PATH",
            "name": "omp_binary",
            "status": "fail",
            "value": "not found"
          }
        ],
        "extension": {
          "current": false,
          "path": "/tmp/glen-example/agent/extensions/glen/index.ts",
          "state": "not_installed",
          "version": null
        },
        "glen": {
          "home": "/tmp/glen-example/glen",
          "home_exists": false,
          "version": "0.1.0"
        },
        "omp": {
          "bin": null,
          "tested_version": "18.14.0",
          "version": null
        },
        "paths": {
          "agent_dir": "/tmp/glen-example/agent",
          "agent_dir_source": "env",
          "base_config_root": "/tmp/glen-example/home/.omp",
          "config_root": "/tmp/glen-example/home/.omp",
          "extensions_dir": "/tmp/glen-example/agent/extensions",
          "profile": null,
          "session_owners_dir": "/tmp/glen-example/home/.omp/run/session-owners",
          "sessions_dir": "/tmp/glen-example/agent/sessions",
          "sessions_dir_source": "agent_dir"
        },
        "skills": {
          "claude": {
            "current": false,
            "path": "/tmp/glen-example/home/.claude/skills/glen/SKILL.md",
            "project": null,
            "state": "not_installed",
            "version": null
          },
          "codex": {
            "current": false,
            "path": "/tmp/glen-example/home/.agents/skills/glen/SKILL.md",
            "project": null,
            "state": "not_installed",
            "version": null
          }
        }
      }
    },
    "hint": "install omp or set GLEN_OMP_BIN",
    "message": "omp is not available"
  },
  "ok": false,
  "v": 1
}
```

<!-- contract: spilled -->
```json
{
  "ok": true,
  "spilled": {
    "bytes": 1707,
    "format": "json",
    "path": "/tmp/glen-example/glen/out/1793346791613-show-5158.json"
  },
  "v": 1
}
```

## Warning codes

| Code | Commands | Meaning |
|---|---|---|
| `omp_version_untested` | doctor | version differs from tested version |
| `invalid_env` | any document, send | invalid output limit or job timeout uses fallback |
| `settle_timeout` | send, wait | persistence settlement timed out |
| `settle_cancelled` | send, wait | settlement cancelled during worker shutdown |
| `persistence_error` | send, wait | omp reported persistence failure |
| `event_filter_rejected` | send, wait | omp rejected optional event filter |
| `skill_newer` | install-skill | newer installed version skipped |
| `skill_modified` | install-skill | digest mismatch skipped |
| `skill_foreign` | install-skill | foreign file, agent mismatch or symlink skipped |
| `skill_sync_failed` | install-skill | file published successfully, but directory sync failed |
| `changed_during_install` | install-skill | destination or ancestor changed after inspection; skipped even with --force |
| `skill_dir_not_empty` | install-skill | removed file, retained nonempty glen directory |
| `not_yet_persisted` | show, tail | live session has not written a transcript yet |
| `extension_version_mismatch` | send | live extension version differs from glen |
| `batched` | send, wait | omp answered this request together with other queued requests |
| `live_turn_continues` | send, wait | live job cancelled; the omp turn keeps running |
| `delivery_unknown` | send, wait | live endpoint never acknowledged the message |
| `socket_path_too_long` | install-extension | GLEN_HOME is too long for a unix socket; live delivery is off |
| `unsafe_path` | install-extension | symlink in the extension path; skipped even with --force |
| `extension_newer`, `extension_modified`, `extension_foreign` | install-extension | file skipped unless --force |
| `extension_dir_not_empty` | install-extension | removed file, retained nonempty glen directory |
| `changed_during_install` | install-extension | extension file or directory changed during install; skipped |

New visible commands and visible aliases must add a section and contract runner in their own wave.
