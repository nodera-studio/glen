---
model: inherit
effort: high
---

# Quick Implementation Orchestrator

<role>
The fast path for SMALL, LOW-RISK changes — a single-file fix, a copy/string tweak, a config
change, a CRUD endpoint that copies an existing one, a missing error handler. You keep the fused
write→test→gate→fix loop (run by the Implementer `routing.sh implementation` names; design-heavy UI goes to the agent `routing.sh ui` names) and the full quality bar,
but drop the planner, advisor, triple-engine review, and conformance gates that the full
`/implement` runs. Same doctrine, smaller blast radius.
</role>

**Done when:** the small change is implemented per the user's ask, `cargo test --locked` + `.claude/scripts/quality-gate.sh`
are green, one fresh review is clean, the codebase MCP is re-indexed, and the work is committed,
pushed, and summarized.

Ground decisions in `.claude/CLAUDE.md` and the relevant area doc from its reference table.

## Scope gate (FIRST — fail-safe auto-classifier, then judgment)

`/implement-quick` is ONLY for changes that are small AND low-risk AND follow an existing pattern.

**Step 0 — run the classifier (mandatory, fail-safe).** Before touching anything:
```bash
bash .claude/scripts/quick-classify.sh "{the user's task description}"
```
Exit 0 = quick path approved. **Any non-zero exit → STOP and route to `/implement`.** The
script escalates on a risk keyword (omp session writes, RPC/bridge/extension, secrets,
releases, `--json` shape changes, dependency changes), a protected path in the diff
(`Cargo.toml`, `Cargo.lock`, `.github/`, `lefthook.yml`, `AGENTS.md`, `CLAUDE.md`,
`.claude/settings.json`, `.claude/hooks/`), or a diff over 6 files / 200 lines, and
**fails safe** (a missing task or a git error also escalates). Echo the script's reason. Do NOT proceed past a non-zero exit; the
classifier is a hard gate, not advice.

Then apply judgment as a backstop — if ANY of these also fires, STOP and route to `/implement`:

- Schema / migration / tenant-isolation change (adapt to this project's ORM/schema layer)
- Auth, crypto, secrets, or shell-execution surface
- Money / billing / any domain-critical business logic that must not regress (this
  project's equivalent of a fiscal/compliance/financial-correctness surface)
- A new queue stage, or a cross-service / module boundary
- Touches 3+ layers, or you cannot name the single pattern file it copies
- Anything you could not fully review from the diff alone

Announce the decision: "Quick path: {one line — why this is small + low-risk + which pattern it
copies}." If a tripwire fires: "This needs the full pipeline — run `/implement`. Reason: {tripwire}."

## Pipeline

### 1 — Implement (fused, routed)

You write no product code: dispatch the Implementer routing names, even on the quick path.
Resolve it first (a non-zero exit is a blocker: report its stderr line, never guess):

```bash
bash .claude/scripts/routing.sh implementation   # claude → impl-coder-claude, codex → impl-coder-codex
bash .claude/scripts/routing.sh ui               # the agent for a design-heavy UI change
# No .claude/scripts/routing.sh in this checkout (cut before harness batch 10): run the main
# checkout's, "$(git rev-parse --path-format=absolute --git-common-dir)/../.claude/scripts/routing.sh";
# neither has it: implementation=claude, ui=impl-coder-ui (the 148655d9 default).
```

A design-heavy UI change goes to the agent `routing.sh ui` names (`orchestrator.md` → UI
implementation routing); everything else to the routed Implementer:

```
Agent tool:
  subagent_type: "impl-coder-claude" | "impl-coder-codex"   (from routing.sh implementation)
  prompt: "Read .claude/agents/implementation/coder-<claude|codex>.md and follow its instructions. Implement this small change (--fresh): {the user's task + the pattern file it copies}."
```

If `impl-coder-claude` is not a loaded agent type (agents load at session start), dispatch
`subagent_type: "general-purpose"` with `model: "opus"` and the same prompt.

Subagents run in the background; continue other work and act on the completion notification,
never on a guess. The Codex Implementer reports failed Codex attempts by SendMessage to `main`
(`orchestrator.md` → Role 1 → "Failed Implementer attempts"). A targeted fix gets exact line
ranges (`orchestrator.md` → Role 2 → "Line-range fix briefs").

Take the file snapshot from `/implement` (`orchestrator.md` → Role 1, "File snapshot") before
this dispatch and check it after: any pre-existing file, tracked or untracked, that is gone is
reported and restored.

The Implementer reads the affected area + the pattern file, makes the change, wires it, handles the error
paths, writes/extends the unit test, runs `cargo test --locked` and
`bash .claude/scripts/quality-gate.sh`, and fixes every failure and finding — looping in ITS context until
tests AND the gate are green. The write→test→fix loop
stays in one context — the routed Implementer's. If it is unavailable (on the Codex route,
"BLOCKER: Codex unavailable"), STOP and report "BLOCKER: <its blocker> — pipeline blocked" to the
user — do NOT hand-write the change, do NOT switch to the other Implementer (that is an owner
routing change) and do NOT reroute to `/implement` (it uses the same routed Implementer).

### 2 — One fresh review pass (the diff only)

Dispatch a SINGLE fresh-context reviewer on the diff — not the full triple-engine; this is the
quick path. Prefer the Codex diff reviewer for an uncorrelated read:

```
Agent tool (subagent_type: codex:codex-rescue):
  prompt: "READ-ONLY review of THIS diff for REAL bugs only — logic, edge cases, missing error
           handling, multi-tenant / fiscal regressions. Not style/lint (the gate owns that).
           If the diff touches rendered output (site/, a TUI, terminal rendering), build and
           run it and look at the output (browser screenshot, or CLI output under a pty)
           before calling it correct; a code trace alone is not enough. Against the real
           ~/.omp run only read verbs (list, show, tail); otherwise point PI_CODING_AGENT_DIR
           at tests/fixtures or a scratch dir under $TMPDIR, or use the fake omp via
           GLEN_OMP_BIN. Never send, steer, follow-up, abort or switch the model of a live
           omp session.
           Do NOT edit or write any file: return the full review as your final message
           text. --wait. Run in the foreground: wait for the Codex task to finish
           and return its full result; never return only a background task id.
           Diff:\n{git diff of the change}"
```

(Or `codex review --base <default-branch merge-base>`.) On a capacity or rate error ("Selected
model is at capacity", 429), retry once after about 60 s; if it fails again, run the Claude
`impl-code-review` pass instead and say in the summary that Codex was unavailable. Never skip the
review silently. Fix any real bug it finds in THIS context.
If the review reveals the change is bigger or riskier than it looked, stop and escalate to
`/implement`.

### 3 — Re-index + ship

**Re-run the classifier before committing** (now the diff exists, so the protected-path + size legs
fire): `bash .claude/scripts/quick-classify.sh "{task}"`. Non-zero → the change grew past the quick path; stop
and escalate to `/implement` rather than committing it on this track.

Re-index the changed files into the **codebase MCP** so search reflects the merged code. Then
commit (HEREDOC format, per this project's commit-trailer convention) and push by default:
`cargo fmt --all -- --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked` → `git push` → open/update the PR into `main` → watch CI (`gh pr checks <n> --watch`).
Never `--no-verify`, never force-push a shared branch. Hand the user a one-paragraph summary +
the `cargo run -- <args>` commands to try the change.

<rules>

- The scope gate is mandatory. When in doubt, bail to `/implement`. The quick path is a SMALLER
  surface, not a lower quality bar — tests + quality gate + one fresh review still hold.
- Keep the generative loop in ONE context (the Implementer's that `routing.sh implementation` names); you write no product code yourself (design-heavy UI goes to the agent `routing.sh ui` names); the reviewer is a FRESH context on the diff only.
- No planner, no advisor, no conformance, no triple-engine — those belong to `/implement`.
- Commit per logical unit, push by default without asking. Never `--no-verify`.
- User-facing message language and error-code/log language per this project's convention.

</rules>
