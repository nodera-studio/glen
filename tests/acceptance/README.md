# W4 acceptance harness (manual, outside CI)

This directory implements Step 9. **Step 11 is deferred**: no real acceptance run has been made on
the W4-only binary. `setup` requires visible `inbox` and `install-extension` and fails with
“acceptance needs W3” until W3 merges. W3b controls are optional. Do not interpret offline fixture
checks as real omp, Claude Code or Codex acceptance.

Prerequisites: Rust toolchain, omp/18.14.0 with the `glen-harness` profile configured
(`.claude/scripts/omp-profile-setup.sh`, its own model and auth), jq, perl, GNU screen, expect for
person mode, Claude Code for b/b2/b3/p/m, Codex plus the `codex:codex-rescue` companion for c1/c2,
shasum, and gitleaks. No download or package upgrade is performed by the harness.
**Close other omp sessions first**, including other omp runs and `omp-engine.sh` reviews under the same profile:
they share the profile's session dir, so the session snapshot could not attribute their changes
to the recorded scratch buckets. Build, version checks and setup progress go to stderr; setup
prints only its scratch path to stdout. Modes p and m need no omp, screen or expect (below).

Final orchestrator decisions override the draft plan's credential-copy design:

- O1 (v0.2 W2): **never export, import or copy credentials, and never use the owner's default
  agent dir**. Every real-omp leg (a, b, b2, b3, c1, c2) runs under profile `glen-harness`
  (`PROFILE=glen-harness` in `acc.sh`): `--profile "$PROFILE"` on every `doctor`, `send`, `ls`
  and `inbox` call and on the `live.sh`/`held.sh` TUIs (`exec omp --profile glen-harness …`); the
  recording wrapper adds `--profile glen-harness` to the real binary when the manifest names the
  profile, and `person.exp` defines `glen() { command glen --profile glen-harness "$@"; }`.
  `setup` dies with "profile glen-harness is missing: run .claude/scripts/omp-profile-setup.sh"
  when the agent dir `doctor --profile glen-harness` reports is not a directory; it never creates
  it. Named profiles ignore `PI_CODING_AGENT_DIR`, so no leg exports it; the manifest keeps
  `agent_dir` as evidence. The project and probe cwds are under `$TMPDIR/ga.XXXXXX`, so new
  sessions land only in the profile's scratch buckets of `$TMPDIR/ga.XXXXXX/project` and
  `…/probe`. Only omp writes session files. The before/after snapshot of the profile's sessions
  dir records names, mtimes and sizes. Existing files must be identical; new files must lie in
  this run's recorded scratch bucket(s). Evidence lists those buckets and the session files left
  there. Sessions are retained; the harness never deletes them.
- Extension installation alone uses `PI_CODING_AGENT_DIR=$S/extension-agent` and **no**
  `--profile`: under a profile it would write the profile's real extensions dir, while the default
  profile with the scratch agent dir writes only under `$S`. The TUI explicitly loads that
  scratch extension with `--no-extensions -e`. The profile's and the owner's extensions are
  untouched.
- Skills install only with `--project` in `$S/project`, initialized as a git repo for Codex
  discovery. Owner skill metadata is compared at teardown. No user skill install is allowed
  through the recording wrapper. `OMP_PROFILE`/`PI_PROFILE` are unset for consistency.
- `GLEN_HOME=$S/home` is under `$TMPDIR` and writable by Codex workspace-write. This is why c2's
  live route can pass; default `~/.glen` cannot take jobs/spills/read markers in that sandbox.
- c2 stays **owner-pending** until explicit chat consent to a temporary network config edit.
  `setup c2` additionally requires `ACC_C2_CONSENT=yes`; that flag records consent already given,
  and is never itself consent. The harness never edits the config.

The setup probe is bounded at 120 seconds; failure is blocked, never passed. L endpoint polling
is bounded at 30 seconds, X lease and talk-back at 120 seconds. Every agent mode has at most 40
calls and a 20-minute budget. Person mode has a 15-minute total and 150-second per-step bound.
Leave setup's printed scratch path available even after a blocked setup so its scratch sessions
can be inspected. A failed setup is not evidence of acceptance.

## Offline checks (safe with this W4-only binary)

```bash
shellcheck tests/acceptance/*.sh
bash -n tests/acceptance/acc.sh
bash -n tests/acceptance/glen-wrapper.sh
bash -n tests/acceptance/self-test.sh
ACC_DRY=1 expect tests/acceptance/person.exp
bash tests/acceptance/self-test.sh
```

The self-test uses hand-made recordings and a fake executable only. It checks a passing b
recording, then missing NONCE_R, an unpaired start and guard_refused (all fail by name); a b3
permission denial after skill load fails AC-420. It also checks wrapper target guards and that
gitleaks detects a generated fake Anthropic-shaped token. For mode p it checks two `woke` (the
task's completion as stream-json's `task_notification` event and as `<task-notification>` XML),
eight `no_wake` (no notification; a blocking `TaskOutput` with and without a later notification; a
`BashOutput` before it; another task's notification; a stopped and a killed task; a foreground glen
output carrying the nonce), a `foreground_only` and a `no_wait` recording (both fail
`AC-206-bg-wait-started`), inbox polling (three calls, or one
without `--wait`/`--peek`, fails `AC-206-no-inbox-polling`), a woken report without the nonce, a
run without a `result` event and a second send; and that the wrapper passes only the whitelisted
manifest `env` names (`LD_PRELOAD` is ignored), accepts P, refuses a foreign handle and a foreign
profile (exit 99), records `wait <job> --for reply --after <id>` and `inbox --wait --timeout 90s`
unchanged, and prepends `--profile glen-harness` only when the manifest names it. For mode m it
generates the recording launcher from a manifest carrying `OMP_PROFILE`, `PI_CONFIG_DIR`,
`CLAUDE_CONFIG_DIR`, `CODEX_HOME` and `LD_PRELOAD`, runs a scripted session through it against a
fake server (bounded by a 10-second alarm, so a launcher that outlives the client's stdin fails) and
checks that the server saw only the allowlisted names, a scratch `GLEN_HOME` and `mcp` as its argv,
and that the wire log equals the session. It then checks a passing wire log (both protocol
versions, with and without `ToolSearch`, and without `m.jsonl` as the Codex leg leaves it), a
`Bash` or other tool in `m.jsonl` (fails `AC-505-no-shell`), a missing `glen_wait`, two `glen_send`,
an `isError:true` send, a protocol version outside the list, a missing `initialize`, a report
without the nonce and a missing recording, and that teardown of a passing run exits 0 with the
wire log in evidence and no owner-session snapshot. Its scratch directory is owned by that
self-test and removed on exit. It never calls setup, person or an actual agent, and runs teardown
only on those hand-made mode m recordings (with `HOME` pointed into its scratch directory).

## Real runs — orchestrator only, after W3 merges

These are instructions for the later Step 11, not a claim that it ran. Use a fresh setup for
each mode. `S` must be the exact path printed by setup. Keep all stdout JSONL transcripts and
copy the final report into `$S/report.md` before check. Evidence root:

```bash
A=tests/acceptance
E=.claude/evidence/w4-acceptance/$(date -u +%Y%m%dT%H%M)
```

(a) Person: a real pty, glyphs and color, no recording wrapper (tee would select JSON).
Setup deliberately leaves project skills absent so person.exp can prove installed/unchanged.
The expect script checks all exits, R/L/X rows, ANSI, reply arrow, nonces, error hint, bounded
send then wait, skills and completions; visible models/abort are also exercised.

```bash
S=$($A/acc.sh setup a)
$A/acc.sh person "$S"
$A/acc.sh check a "$S"
$A/acc.sh teardown "$S" "$E/a-person"
```

(b) Fresh **general-purpose** subagent (never fork): setup b, dispatch exactly
`$S/prompt-b.md` as its entire prompt, save final output to `$S/report.md`. The installed Claude
variant is embedded. This subagent inherits the invoking repo's CLAUDE.md/AGENTS.md. Find its
new `agent-*.jsonl` transcript under `~/.claude/projects/…` and copy to `$S/transcript.jsonl`.
The transcript is mandatory: absent means isolation unverified, never pass. The checker
rejects tools besides Bash or Read under `$S` and obvious shell source/web reads. The
orchestrator must also read every tool call to reject any repo/docs/web access, including
indirect shell reads. Then check b and teardown to `$E/b-claude`.

(b3) Discovered skill, clean context outside this repo; no pasted skill, no `/glen`, no command
recipe in the prompt. The wrapper named glen is first on PATH. Skill/Read loading must be in
the transcript. Any denied glen Bash call fails, including one before skill invocation;
allowed-tools grants apply only to the invoking turn.
Claude Code 2.1.309 supports the scoped `--allowedTools "Skill(glen)"` grant under
`dontAsk`; this run uses it to permit loading only glen. Every Skill call must name glen.
Bash is deliberately NOT allowed on the command line: glen's Bash permission must come only
from the skill's own `allowed-tools: Bash(glen *)` grant, not from the harness.

```bash
S=$($A/acc.sh setup b3)
(cd "$S/project" && PATH="$S/bin:$PATH" claude -p "$(cat "$S/prompt-b3.md")" \
  --permission-mode dontAsk --allowedTools "Skill(glen)" --output-format stream-json --verbose) > "$S/b3.jsonl"
jq -r 'select(.type=="result") | .result' "$S/b3.jsonl" > "$S/report.md"
$A/acc.sh check b3 "$S"
$A/acc.sh teardown "$S" "$E/b3-claude-discover"
```

(b2) Deterministic permission probe and control. Success requires an actual successful glen
Bash tool result in the probe and a denied matching Bash call in the control. An un-denied
control, glen not found, or missing matching tool use is **inconclusive**, never passed.
An existing personal glen skill may make the control inconclusive; do not remove owner skills.

```bash
S=$($A/acc.sh setup b2)
(cd "$S/project" && PATH="$S/bin:$PATH" claude -p '/glen list the omp sessions of this project' \
  --permission-mode dontAsk --output-format stream-json --verbose) > "$S/b2-probe.jsonl"
mkdir "$S/noskill"
(cd "$S/noskill" && PATH="$S/bin:$PATH" claude -p "Run the shell command: glen ls --cwd $S/project" \
  --permission-mode dontAsk --output-format stream-json --verbose) > "$S/b2-control.jsonl"
$A/acc.sh check b2 "$S"
$A/acc.sh teardown "$S" "$E/b2-allowed-tools"
```

(c1) Codex sandbox network off: setup c1. Dispatch **codex:codex-rescue**, requesting a fresh
foreground task with companion flags `--write --cwd $S/project --prompt-file $S/prompt-c1.md`.
Check the forwarded cwd and prompt file in its output. Save that output as `$S/report.md`.
The report must name `$S/project/.agents/skills/glen/SKILL.md` and quote the sandbox hint.
Reads must succeed; every send must refuse within 5 seconds, network false; jobs must not grow.
Then check c1 and teardown to `$E/c1-codex-sandboxed`.

If Codex fails to discover the skill, rerun once with cwd confirmed. Still absent: record the
finding, use an embedded variant only as separately named `c1-embedded` evidence, and open a
follow-up. Do not count it as discovered-skill acceptance.

(c2) Only after explicit owner consent: back up `~/.codex/config.toml` with `cp -p` to a private
temporary directory. If `[sandbox_workspace_write]` already exists, edit network_access by
hand; do not add a duplicate table. Set network_access=true temporarily. Record the original
backup SHA256 in `$S/config.before.sha256` after `ACC_C2_CONSENT=yes $A/acc.sh setup c2`.
Dispatch as c1, using prompt-c2.md. Restore the config from the backup **before check**, and use
`cmp` to prove byte identity, even on failure/interruption (use a shell trap). The checker
independently hashes the restored file. Do not copy the config into evidence. c2 requires a
live L reply and RPC R refusal with network:true. Teardown to `$E/c2-codex-network`.
If consent is absent, record `owner-pending` / `not run: no consent`.

(p) `claude -p` push evidence (v0.2 W2, AC-v2-206). No real omp: `setup p` builds `fake_omp`,
copies the fixture agent dir to `$S/omp-agent`, moves fixture session P (`e27c0de`) into the
bucket glen assigns `$S/project`, and bakes `HOME=$S/omp-home`, `PI_CODING_AGENT_DIR`,
`GLEN_OMP_BIN` and `FAKE_OMP_SCENARIO=progress`, `FAKE_OMP_DELAY_MS=100000` into the manifest's
`env`, which the wrapper exports (only those names) for every glen call. The fake reply arrives
about 100 seconds after the send. The prompt (`prompts/claude-push.md`) is uncoached: it never
names the receiver or how to run it. Give the `claude -p` call a 600-second shell timeout.

```bash
S=$($A/acc.sh setup p)
(cd "$S/project" && PATH="$S/bin:$PATH" claude -p "$(cat "$S/prompt-p.md")" \
  --permission-mode dontAsk --allowedTools "Skill(glen)" --output-format stream-json --verbose) > "$S/p.jsonl"
jq -r 'select(.type=="result") | .result' "$S/p.jsonl" > "$S/report.md"
$A/acc.sh check p "$S"
# The fake job may still run after claude exits: wait up to 150 s for no active job.
for _ in $(seq 150); do
  [[ $(env GLEN_HOME="$S/home" "$S/glen-bin/glen" jobs --active --json | jq '.data.jobs | length') == 0 ]] && break
  sleep 1
done
$A/acc.sh teardown "$S" "$E/p-claude-push"
```

Checks: `paired_calls`, `guard_refused`, `call_budget`; `AC-206-send` (exactly one counted send,
to P); `AC-206-bg-wait-started` (a Bash tool_use with `run_in_background: true` runs `glen wait`
with `--for reply`); `AC-206-no-inbox-polling` (at most two `inbox` calls, each with `--wait` or
`--peek`); `AC-206-evidence` (a `result` event exists); `AC-206-nonce` (a woken run's report
quotes `NONCE_P`). The `AC-206-evidence` string carries the outcome: `wake=<class>
launch_event=<i> task=<id> delivered_event=<i> resumed_event=<i> first_result_event=<i>
result_event=<i> foreground_glen_output_had_nonce=<bool> fetched_before_delivery=<bool>` (0-based
indices into `p.jsonl`, so line = index + 1; `task` is the background task id from the launch
result; `resumed_event` is the first assistant text or tool_use after the notification;
`first_result_event` and `result_event` are the first and the last `result`). Classes: `woke` (a
background reply wait was launched, its task's own completion notification, a `system`
`task_notification` event or a user `<task-notification>` with status `completed` or `failed` (a
`glen wait` that exits 10 is `failed`), arrives, and an assistant text or tool_use follows it; no
foreground glen output carried `NONCE_P`, no tool result carried it before that notification, and
nothing retrieved the task's output before it, so a `TaskOutput` or `BashOutput` naming the task or
a `Read` of its output file is not a wake), `no_wake` (launched, but no such notification, only
another task's, a stopped or killed task, a retrieval first, or no assistant activity after the
notification), `foreground_only` (a reply wait without
`run_in_background`) and `no_wait` (none). A `result` before the notification does not rule out
`woke`: Claude Code 2.1.316 keeps a `claude -p` process with a running background task alive past
its first `result`, then resumes the run on the notification with a new `init`, and a second
`result` ends it (recorded 2026-11-02). The class decides the skill's `claude -p` wording
(plan Step 13); `no_wake` passes the checks but is not push. Read every tool call of `p.jsonl`
as for b/b3. Teardown keeps gitleaks and the owner-skills comparison and skips the owner-session
snapshot (mode p touches only scratch sessions).

(m) MCP evidence (v0.2 W5, AC-v2-505). No real omp and no profile: `setup m` builds `fake_omp`
and prepares the scratch project, fixture session P (`e27c0de`) and `HOME=$S/omp-home` as `setup p`
does, with `FAKE_OMP_DELAY_MS=8000` (the job takes about 8 seconds, so a model that sends without
waiting must call a wait) and `GLEN_HOME=$S/home`. It writes the recording launcher
`$S/bin/glen-mcp`: `exec env -i PATH=/usr/bin:/bin <names> "$S/glen-bin/glen" mcp`, its stdin and
stdout teed into `$S/mcp-in.ndjson` and `$S/mcp-out.ndjson` (the wire log). Only manifest `env`
names `HOME`, `PI_CODING_AGENT_DIR`, `GLEN_OMP_BIN` and `FAKE_OMP_*` pass and `GLEN_HOME` is fixed
under scratch, so `OMP_PROFILE`, `PI_PROFILE`, `PI_CONFIG_DIR`, `CLAUDE_*`, `CODEX_*` and API keys
never reach the server or its children; that closed environment, not the recording wrapper's
guard, is what makes all 14 tools safe here. Setup also writes `$S/mcp.json` (for Claude Code; its
`env` repeats the same names), `$S/codex-mcp.toml` (the `[mcp_servers.glen]` table for Codex, same
`command` and `env`, `tool_timeout_sec = 120`) and the uncoached `prompt-m-claude.md` and
`prompt-m-codex.md` (`prompts/claude-mcp.md`, `prompts/codex-mcp.md`), which name neither the
protocol, the tools nor a shell. `calls.ndjson` stays empty (there is no recording wrapper), so the
`json_contract` evidence step below is vacuous for mode m. Give the `claude -p` call a 300-second
shell timeout.

```bash
A=tests/acceptance
E=.claude/evidence/v020-wave-5-mcp/$(date -u +%Y%m%dT%H%M)
S=$($A/acc.sh setup m)
(cd "$S/project" && claude -p "$(cat "$S/prompt-m-claude.md")" --mcp-config "$S/mcp.json" --strict-mcp-config \
  --allowedTools "mcp__glen__glen_ls,mcp__glen__glen_send,mcp__glen__glen_wait" \
  --permission-mode dontAsk --max-budget-usd 1.00 --output-format stream-json --verbose) > "$S/m.jsonl"
jq -r 'select(.type=="result") | .result' "$S/m.jsonl" > "$S/report.md"
$A/acc.sh check m "$S"
# The fake job may still run after claude exits: wait up to 60 s for no active job.
for _ in $(seq 60); do
  [[ $(env GLEN_HOME="$S/home" "$S/glen-bin/glen" jobs --active --json | jq '.data.jobs | length') == 0 ]] && break
  sleep 1
done
$A/acc.sh teardown "$S" "$E/m-claude-mcp"
```

`--strict-mcp-config` keeps the owner's MCP servers (Mnemosyne included) out of the run;
`--permission-mode dontAsk` with only the three `mcp__glen__` tools allowed denies Bash, Skill,
Edit and Task; the run has no `PATH` shim, and an owner-installed `glen` is unreachable because
Bash is denied. If the model needs `ToolSearch` to load the MCP tool schemas, add exactly
`ToolSearch` to `--allowedTools` and record it. After teardown, confirm that no `glen mcp`,
`glen __worker` or `fake_omp` survives for the scratch home.

Checks (each `tools/call` is matched to its answer by `id` in the wire log): `AC-505-initialize`
(an `initialize` request was recorded and its `protocolVersion` is `2025-11-25` or `2025-06-18`;
evidence `client=<name>/<version> protocol=<v> first_method=<m>`), `AC-505-list` (a `tools/list`
was answered), `AC-505-ls` (a `glen_ls` answered `isError:false`), `AC-505-send` (exactly one
`glen_send`, its `arguments.session` P's handle or id and `arguments.message` `NONCE_M`, answered
`isError:false`), `AC-505-wait` (a `glen_wait` of that send's job handle or id answered
`isError:false`), `AC-505-nonce` (the report quotes `NONCE_M`) and `AC-505-no-shell` (every
`tool_use` in `m.jsonl` is `mcp__glen__glen_ls`, `mcp__glen__glen_send`, `mcp__glen__glen_wait` or
`ToolSearch`; without `m.jsonl`, as in the Codex leg, the wire log alone decides). The evidence
strings of `AC-505-ls`, `-send` and `-wait` carry each call's exact `arguments` object, which shows
whether the client fills schema defaults. Mode m has no `paired_calls`, `guard_refused` or
`call_budget`. Teardown keeps gitleaks and the owner-skills comparison, skips the owner-session
snapshot (mode m touches only scratch fixture copies) and copies `m.jsonl` and the wire log into
evidence.

Record in `$E/README.md`, reading `AC-505-initialize` first: the client's `initialize`
`protocolVersion` and `clientInfo` and the first method from `mcp-in.ndjson`; `claude --version`,
the git SHA and the binary sha256 (`provenance.json`); the `result` event's `num_turns` and
`total_cost_usd` (expected `num_turns` ≤ 6; a run over 6 turns or $1.00 is reported, not repeated);
and each tool call's exact `arguments`. Read every tool call of `m.jsonl` as for b and b3. At most
one rerun for model nondeterminism, recorded, and only if a check failed because the agent misread
a tool description (then fix that tool's `note` in `docs/json.md`, regenerate the catalog and keep
both wordings in the evidence). If the client probed `server/discover` and never fell back, or
refused the server for its protocol, stop: AC-v2-505 fails for that client, and supporting the
stateless revision is a scope change to report to the owner, not to make here.

The Codex leg is **owner-pending** by default. `setup m` writes `$S/codex-mcp.toml` and
`prompt-m-codex.md` as material the owner runs by hand, after which `check m "$S"` reads the wire
log that run left in `$S`. `acc.sh` reads no `ACC_*_CONSENT` for mode m and never touches
`~/.codex`. The orchestrator records `owner-pending / not run: no route that leaves ~/.codex
untouched` unless a read-only inspection (`codex --help`, `codex mcp --help`, the
`codex-companion.mjs task --help` text) finds a documented way to supply `[mcp_servers.glen]` to one
run without writing `~/.codex/config.toml`, `$CODEX_HOME` or any credential file and without
copying a credential; then it dispatches `codex:codex-rescue` with `--write --cwd $S/project
--prompt-file $S/prompt-m-codex.md` and that route, and runs `check m`. Owner: register
`[mcp_servers.glen]` from `$S/codex-mcp.toml` yourself, run `prompt-m-codex.md` in `$S/project`,
then `acc.sh check m "$S"` and `acc.sh teardown "$S" "$E/m-codex-mcp"`.

For every agent mode, before teardown, also run:

```bash
GLEN_CONTRACT_EVIDENCE="$S/calls.ndjson" cargo test --test json_contract -- --ignored evidence_shapes
```

Mode b/b3 must list R/L/X, read R, obtain RPC R and live L nonces (following running jobs with
wait), read inbox's T nonce, send to X once or twice (exit 6), quote all nonces and X hint,
and never invoke install-extension. No unpaired start, guard refusal or killed call may pass.
If an agent misreads the skill, fix the template and record both wordings; if glen fails, add
a regression test before fixing code. At most one model-nondeterminism rerun, recorded.

## Recordings, safety and teardown

The wrapper records paired `{t:"start",id,argv,ts}` / `{t:"end",id,argv,exit,ms,stdout}` NDJSON.
Stdout is parsed JSON where possible or a string, capped only in the recording at 256 KiB.
Logging failures preserve the subprocess exit. Targets are manifest IDs or hex suffixes,
`@new` with canonical cwd under `$S`, or `@latest` in `$S/project`; controls use the same guard.
Directory traversal/symlink cwd escapes and user-scope skill installation are refused (exit 99).
Every glen invocation sets GLEN_HOME under scratch; the `glen-harness` profile's auth stays in
place, never copied. Mode p's manifest `env` passes only `HOME`, `PI_CODING_AGENT_DIR`,
`GLEN_OMP_BIN` and `FAKE_OMP_*`; any other name is ignored. Mode m has no wrapper: its launcher
starts `glen mcp` under `env -i` with the same names plus a scratch `GLEN_HOME`, and its wire log
holds protocol messages only, never environment values.

`check` is read-only with respect to agents/sessions and writes only verdict evidence. Missing
transcript or missing nonce fails; unpaired start means a killed call. `verdict.json` names
every criterion and its evidence. b2's failed control probe is inconclusive, not permission proof.

Teardown sends `/exit`, waits 3 seconds then closes remaining named ga-* screens. It captures
screens, requires no live endpoint files, compares session and owner-skill snapshots, copies
recordings/prompts/reports/provenance/verdict into evidence, and runs `gitleaks dir --no-banner
--redact` (no git history). Any finding fails the mode. Provenance holds git SHA, binary SHA256,
tool versions and env **names only**, never values. Never include credential stores in evidence.
Evidence `transcript.jsonl` and verdict input retain only `user`, `assistant`, `system`, and `summary` lines, excluding `api-request` telemetry.
Scratch extension/skills/home remain in TMPDIR for inspection; session files remain in the
`glen-harness` profile's scratch bucket(s) (mode p: under `$S/omp-agent`). The harness never removes omp data or owner credentials.

Create an evidence README with per-mode verdicts, tool versions, retained scratch bucket paths,
and limitations. A missing runtime/tool/network/credential is blocked/unverified, never passed.
Step 11 completion, not this implementation, is what allows W4's board issue to move to done.
