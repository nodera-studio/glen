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
Cursor login detection, the final model id, and the predicted `mcp__mnemosyne_memory_*`
tool names await the owner's live login. The placeholder default is
`cursor/gemini-3.1-pro`; setup validates availability, and `--model cursor/<id>` selects another.

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
