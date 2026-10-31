---
name: omp
description: >-
  Dispatch the omp engine as a third reviewer, third engine, or optional omp investigator.
  Use for review/debug orchestrators needing a third engine or requests to ask the omp engine.
  NOT for talking to the owner's interactive omp sessions; use the existing glen skill for those.
allowed-tools: Bash
---

# omp engine dispatch

## Role

omp is a third reviewer and an optional third investigator only, read-only, on Cursor in
the `glen-harness` profile. It has no supervisor or plan-judge role.

## One-time setup

The owner runs `omp --profile glen-harness login cursor`, then
`.claude/scripts/omp-profile-setup.sh`, then `.claude/scripts/omp-profile-setup.sh --check`.
Cursor login detection and the predicted `mcp__mnemosyne_memory_*` tool names await
the owner's live login. Setup picks the first id present in the live Cursor model list from
an ordered ladder, each entry with its own startup thinking level: `cursor/gemini-4-argon`
at medium, then `cursor/gemini-3.8-flash` at medium, then `cursor/grok-4.7` at high. It
writes `modelRoles.default` as `<id>:<level>` (omp 18.14.0 honours this suffix itself,
`config/model-resolver.ts` `splitThinkingSuffix`; host-verified against the session
transcript's own `thinkingLevel` field, not the model's self-reported, unreliable answer).
The engine's per-dispatch `--thinking <level>` on each `glen send` (mirrored in
`omp-engine.sh`'s own `MODEL_LADDER`/`THINKING_LADDER`, shared with setup's from
`.claude/omp/model-ladder.sh`) is belt and braces on top of the persisted default, not the
only mechanism. If none of the three ids is present, omp is unavailable, with a reason
naming all three. `--model cursor/<id>` selects another model and requires that exact id; it
does not use the ladder, and setup never appends a thinking-level suffix for it.

## How to invoke

Run the engine as background Bash from orchestrators so other engines can work concurrently:

```bash
.claude/scripts/omp-engine.sh probe
.claude/scripts/omp-engine.sh review --repo DIR --base REF --out FILE --scope TEXT --deadline 15m
.claude/scripts/omp-engine.sh review --repo DIR --uncommitted --out FILE
.claude/scripts/omp-engine.sh debug --repo DIR --brief BRIEF --out FILE --base REF --deadline 20m
```

`--repo` defaults to the current directory. Review requires exactly one of `--base` and
`--uncommitted`; debug's `--base` is optional. `--keep` retains the snapshot on terminal runs.
Exit codes: 0 completed (probe: ready), 3 skipped, 4 failed, 5 timeout, 2 usage error.
Each public mode prints one JSON line; diagnostics go to stderr. Review/debug also write
`FILE` (final answer) and `FILE.json` (result and recovery metadata). Internal smoke helpers
are `snapshot SRC DEST` (JSON), `policy SRC` (source label followed by YAML), and
`envcheck -- CMD...` (the command's stdout through the dispatch environment).

## What is enforced

The policy overlay sets `tools.approvalMode: always-ask` and per-tool deny/allow rules.
Writes fail closed in headless approval because glen passes `--no-ui`. Reads are contained
by `fetch.enabled: false`, `web_search.enabled: false`, and `tools.restrictedFolders: ["~"]`.
Mnemosyne memory is read-only; every memory call uses `project: "glen"`.

Dispatch scrubs `CLAUDE_CONFIG_DIR`, `PI_PY`, `PI_JS`, `PI_CODING_AGENT_DIR`,
`OMP_CODING_AGENT_DIR`, `OMP_PROFILE`, and `PI_PROFILE`. It sets a private `GLEN_HOME`,
the captured policy's absolute `PI_CONFIG_FILES`, and the job deadline. The Mnemosyne token
passes through untouched; never print its value or any other environment values.

The engine sees a snapshot outside HOME, without Git metadata. Root agent configuration
is relocated to `_agent-config/` (for example `.claude/` becomes `_agent-config/claude/`).
Policy comes from `main` when available, with a warned worktree fallback.

## Best-effort Claude Code deny rules

`.claude/settings.json` denies literal command prefixes such as `omp login` in Claude's
own typed Bash calls. These are courtesy guardrails, not a security boundary or a sound
shell parser: do not rely on them to catch `bash -c "omp login"`, heredocs, variable
indirection, or other alternate invocation forms. They do not govern calls inside scripts.
[Claude Code also checks separated subcommands and unwraps some wrappers](https://code.claude.com/docs/en/permissions#compound-commands);
prefix matching is not limited to position zero of the entire Bash call.
The real default-profile protections are explicit `--profile glen-harness` on every
harness invocation, setup's default-profile fingerprint, and the OS sandbox.

## Risks

The macOS `sandbox-exec` launcher contains reads of other processes' sibling directories
under `$TMPDIR`; containment does not rely solely on file permissions. Any omp invocation
can write shared, cross-profile state in `~/.omp/natives/` and `~/.omp/run/`; the
default-profile fingerprint excludes both. Setup's `--dry-run` makes no config changes, but
querying config still initializes databases and caches on disk under `glen-harness`.

**The engine is unavailable on Linux in v0.1.0.** bwrap cannot isolate abstract Unix sockets
while still letting the sandboxed process reach Cursor over the network, so there is no safe
bwrap policy today. `probe` returns `unavailable: linux abstract-socket isolation not
supported` and dispatch never runs or calls `omp-sandboxed-linux.sh`. That launcher stays in
the repo for a later Landlock-based version.

**Outbound network is restricted to TCP port 443 plus the mDNSResponder Unix socket**
(fix cycle 4), not to the Cursor API host specifically. Two residuals remain, accepted by the
owner: any HTTPS host on the public internet is reachable from inside the sandbox (not just
Cursor's), and `agent.db` (the Cursor credential store) stays readable by the sandboxed omp
process, since omp needs it to authenticate. Neither is mitigated further in v0.1.0.

## Results

Read both `FILE` and `FILE.json`: status/reason, job, session, model/provider, snapshot hash,
elapsed time, replies, and summed token/cost figures. Usage and cost are notional on a Cursor
subscription; the owner can check quota with `omp --profile glen-harness usage`.

## Failure handling

Record exit 3 as a skip with its reason. Capacity failures retry once after 60 seconds only
when at least five minutes of the original deadline remain. Exit 5 means timeout; the tree
is retained because the job may still be running. Old runs are pruned after one day.
Malformed output remains in FILE and has `status: "failed", reason: "malformed output"`
in FILE.json, but exits 0 when execution completed cleanly. Give that result no vote.
Never block the whole gate on omp failure; report its absence and continue with other engines.
`GLEN_OMP_ENGINE=off` disables the engine.

A killed run can be recovered from FILE.json as soon as it records `status: "running"`.
A `timeout` result is recoverable only when FILE.json contains a non-null `job` handle;
an interruption before any job handle was obtained is `failed`, not `timeout`.
Use its `glen_home`, `job`, `run_dir`, and deadline with `glen wait`; never re-send.
Apply the same environment scrubbing and policy path when recovering:

```bash
env -u CLAUDE_CONFIG_DIR -u PI_PY -u PI_JS -u PI_CODING_AGENT_DIR \
  -u OMP_CODING_AGENT_DIR -u OMP_PROFILE -u PI_PROFILE \
  GLEN_HOME="<glen_home>" PI_CONFIG_FILES="<run_dir>/policy.yml" GLEN_JOB_TIMEOUT=15m \
  "$GLEN_BIN" wait "<job>" --timeout 60s --json
```

## Inspecting a run

`"$GLEN_BIN" --profile glen-harness show <session path> --tools full`

## After an omp upgrade

Rerun `omp-profile-setup.sh --check` and re-verify the enforcement mechanism against the
new omp version. See `docs/design.md` for the underlying design rather than assuming parity.

## Rules

Never use `omp -p`, bare `omp --mode rpc`, or bare `glen send` for engine work. Always use
the engine script and the `glen-harness` profile; never use the default omp profile.

## Critical evaluation

omp is a peer engine, not an authority. Scrutinize its findings as carefully as Claude's
or Codex's, verify the evidence, and resolve disagreements from the code and contracts.
