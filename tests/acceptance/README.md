# W4 acceptance harness (manual, outside CI)

This directory implements Step 9. **Step 11 is deferred**: no real acceptance run has been made on
the W4-only binary. `setup` requires visible `inbox` and `install-extension` and fails with
“acceptance needs W3” until W3 merges. W3b controls are optional. Do not interpret offline fixture
checks as real omp, Claude Code or Codex acceptance.

Prerequisites: Rust toolchain, omp/18.14.0 with a working normal model/auth configuration, jq,
perl, GNU screen, expect for person mode, Claude Code for b/b2/b3, Codex plus the
`codex:codex-rescue` companion for c1/c2, shasum, and gitleaks. No download or package upgrade is
performed by the harness. **Close other omp sessions first**, so the owner-session snapshot
can attribute all changes to the recorded scratch buckets. Build, version checks and setup progress go to stderr; setup prints
only its scratch path to stdout.

Final orchestrator decisions override the draft plan's credential-copy design:

- O1: **never export, import or copy credentials**. omp uses the owner's normal agent dir and
  auth, with scratch project/probe cwd under `$TMPDIR/ga.XXXXXX`. Only omp writes session files.
  The before/after snapshot records names, mtimes and sizes. Existing files must be identical;
  new files must lie in this run's recorded scratch bucket(s). Evidence lists those buckets
  and the session files left there. Sessions are retained; the harness never deletes them.
- Extension installation alone uses `PI_CODING_AGENT_DIR=$S/extension-agent`; the TUI explicitly
  loads that scratch extension with `--no-extensions -e`. The owner's extensions are untouched.
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
gitleaks detects a generated fake Anthropic-shaped token. Its scratch directory is owned by
that self-test and removed on exit. It never calls setup/person/teardown or an actual agent.

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

```bash
S=$($A/acc.sh setup b3)
(cd "$S/project" && PATH="$S/bin:$PATH" claude -p "$(cat "$S/prompt-b3.md")" \
  --permission-mode dontAsk --output-format stream-json --verbose) > "$S/b3.jsonl"
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
Every glen invocation sets GLEN_HOME under scratch; normal omp auth stays in place, never copied.

`check` is read-only with respect to agents/sessions and writes only verdict evidence. Missing
transcript or missing nonce fails; unpaired start means a killed call. `verdict.json` names
every criterion and its evidence. b2's failed control probe is inconclusive, not permission proof.

Teardown sends `/exit`, waits 3 seconds then closes remaining named ga-* screens. It captures
screens, requires no live endpoint files, compares session and owner-skill snapshots, copies
recordings/prompts/reports/provenance/verdict into evidence, and runs `gitleaks dir --no-banner
--redact` (no git history). Any finding fails the mode. Provenance holds git SHA, binary SHA256,
tool versions and env **names only**, never values. Never include credential stores in evidence.
Scratch extension/skills/home remain in TMPDIR for inspection; session files remain in the
normal agent dir's scratch bucket(s). The harness never removes omp data or owner credentials.

Create an evidence README with per-mode verdicts, tool versions, retained scratch bucket paths,
and limitations. A missing runtime/tool/network/credential is blocked/unverified, never passed.
Step 11 completion, not this implementation, is what allows W4's board issue to move to done.
