---
name: codex
description: >
  Dispatch a task to OpenAI Codex through the installed `codex:codex-rescue` plugin
  agent. Use when the user wants a second-opinion review, a deep-dive on a hard
  bug, an independent implementation pass, or any time they type `/codex <prompt>`.
  Also use proactively when Claude is stuck on a substantial debugging or
  implementation problem and a different model's perspective is likely to help.
  Triggers on: "ask codex", "second opinion", "have codex look at this", "fire
  codex", "/codex", being stuck on a multi-step bug, wanting parallel investigation,
  review-implementation orchestrators needing the Codex engine.
allowed-tools: Agent, Bash
argument-hint: '<task or question to dispatch to Codex>'
---

# Codex Dispatch

## Roles come from routing

Who does each kind of work is set by `.claude/routing.json`, read through
`bash .claude/scripts/routing.sh <key>` (an owner-confirmed override in
`.claude/auto-mode/routing.json` wins). Today (decision 148655d9) `implementation` is
`claude`: the Claude Implementer (`coder-claude.md`) writes product code, and Codex reviews
it. `review`, `planning`, `debug` and `audits` include `codex`. Design-heavy UI (`site/`,
landing pages, web (non-Rust) visual components) goes to the agent `routing.sh ui` names, and
Codex reviews it (`implementation/orchestrator.md` → UI implementation routing). Codex fills
these roles:

1. **Review engine** — one of the three engines in the Role 2 / `/review-implementation`
   review, via the native **`codex review --base <BASE>`** diff reviewer, and the second
   engine of the lean `/code-review` gate via `codex:codex-rescue`.
2. **Advisor** — the **Step 4.5 advisor gate** in `/implement` and `/codex-advisor`:
   a read-only adversarial critique of the approved plan (verdict/risks/steelman) via
   the **`codex:codex-rescue`** agent — NOT `codex review`, NEVER `codex exec`.
   **Synthesize, never forward**; ONE reconcile call on disagreement; escalate
   unresolved CRITICAL risks; gate the call rate.
3. **Planning, debug and audit engine** — the second engine of the dual-engine architect and
   planner (`/plan-implementation`), the `/deep-debug` investigator pair and the audit
   orchestrators, via `codex:codex-rescue`.
4. **Implementer, only when `routing.sh implementation` prints `codex`** — the fused
   write → wire → harden → test → gate → fix loop for `/implement` (Role 1) and
   `/implement-quick`, run in Codex's own context via the **`coder-codex.md`** agent
   (`codex-companion.mjs task --json --write`). Codex reads its conventions from the
   repo-root `AGENTS.md`. This is not the thin `codex:codex-rescue` forwarder — it owns the
   `touchedFiles`/completion-report contract the orchestrator needs. When routing prints
   anything else, `coder-codex.md` stops at its Step 0 and `impl-coder-claude` runs instead.

`codex exec` is never used — Codex is invoked only through the plugin runtime
(`coder-codex.md` → `codex-companion.mjs`, `codex:codex-rescue`, or `codex review`).

Hand a task to OpenAI Codex through the `codex:codex-rescue` plugin subagent. This
skill is a procedural guide — it tells you HOW to invoke Codex, not what Codex
should do.

## When to use

- **Second opinion / dual-engine review** — you want a different model to look at
  the same diff, design, or root-cause hypothesis.
- **Stuck on a hard bug** — Claude has tried 2+ approaches and isn't converging.
- **Substantial implementation handoff** — a multi-step coding task the user asks
  Codex to take. Harness implementation follows `routing.sh implementation`, not this skill.
- **Parallel passes** — review-implementation needs N Codex passes alongside N
  Claude passes (see Multi-pass section).

## When NOT to use

- Trivial lookups, single-file edits, anything Claude can finish in <60 seconds.
- Repository inspection, file reading, status-checking — do those yourself first
  and only forward a tight, well-scoped prompt to Codex.
- Quick clarifications — just ask the user.

## How to invoke (primary path)

Use the **Agent tool** with `subagent_type: "codex:codex-rescue"`. Do NOT use the
Skill tool to call Codex (the wrapping skill names are not indexed in every
session). **Never call `codex exec`** — Codex is invoked only through the plugin: the
`codex:codex-rescue` agent (open-ended tasks / diagnosis) or the native `codex review`
reviewer (diff review, or `--uncommitted` critique of written material).

Pass the user's task verbatim in the `prompt` field. The `codex-rescue` agent is
a thin forwarder; it strips routing flags and shapes the prompt itself. Routing
flags the user may include and that you should preserve in the prompt text:

- `--background` / `--wait` — execution mode (default: foreground for small tasks)
- `--model gpt-5.4` | `gpt-5.3-codex` | `gpt-5.3-codex-spark` (or `spark`)
- `--effort high` | `medium` | `low` (default: leave unset)
- `--write` / read-only (default: write-capable unless user wants review only)
- `--resume` / `--fresh` (default: fresh; `--resume` continues prior Codex work)

### Foreground by default for agent briefs

Every harness brief to `codex:codex-rescue` (plan, implement, review, `/code-review`,
`/implement-quick`, `/codex-advisor`, `/deep-debug` and the audit orchestrators) MUST include
`--wait` and say: "Run in the foreground: wait for the Codex task to finish and
return its full result as your final message; never return only a background task id." Without
`--wait` the forwarder backgrounds any task that looks long and returns at once with a task id
("file doesn't exist yet"), leaving the caller to poll. If a task id comes back anyway, fetch it
(`node "$COMPANION" status <id> --wait`, then `node "$COMPANION" result <id>`) before using the
pass. Use `--background` only when the user asks for it; a user's own `/codex` request keeps
whatever mode they chose.

### Artifacts come back as text, never as files

The `codex:codex-rescue` sandbox is workspace-write and does not escalate: Codex can write
only inside its own cwd, which follows your shell's current directory at dispatch time (often
a worktree). A planner told to write its plan into the main checkout's
`.claude/plans/<slug>.private/plan-2.md` failed on that write. So every codex-rescue brief that
produces an artifact (planner, architect, reviewer, auditor, advisor) says "do NOT write any
file; return the full artifact as your final message text", and never names an output path
outside Codex's cwd (in practice: names none). The orchestrator saves the returned text where
it belongs. This does not apply to the Implementer (`coder-codex.md`), whose job is writing
product code inside the worktree it runs in.

### Cancelling a Codex job (TaskStop is not enough)

Stopping the Claude agent that launched Codex (TaskStop on `impl-coder-codex`, or a
`codex:codex-rescue` agent with `--write`) does not stop Codex. The companion job, its
`app-server-broker.mjs serve --cwd <worktree>` broker, the `codex app-server` and its sandbox
processes keep running and keep editing the worktree. Whenever you stop a Codex-backed agent:

0. Guard first: `<worktree>` must be set, absolute, a worktree root (`git -C <worktree>
   rev-parse --show-toplevel` equals it) and not the main checkout. The companion turns an
   empty `--cwd` into its own cwd, so an unset path would cancel the main checkout's jobs. The
   guard is step 0 of the recipe in `coder-codex.md`.
1. `env -u CODEX_COMPANION_SESSION_ID node "$COMPANION" status --cwd <worktree> --json`
   (every session's jobs), then for each id in `.running[]`:
   `env -u CODEX_COMPANION_SESSION_ID node "$COMPANION" cancel <id> --cwd <worktree> --json`
   (interrupts the turn and kills the job's process tree). `COMPANION` is resolved below.
2. If the worktree still changes, kill that worktree's broker tree only. The exact recipe
   matches only a node process running `app-server-broker.mjs serve`, then `--cwd <worktree>
   --pid-file` as a fixed string, so neither your own shell nor other worktrees' brokers are
   hit. It is in `.claude/agents/implementation/coder-codex.md` → "Cancelling a run".
3. Confirm `git -C <worktree> status --short` is unchanged 30 s later before reusing it.

The broker also survives a run that ended normally and the removal of its worktree, and a
`codex-companion.mjs task` process can outlive its finished job. Remove a worktree any Codex
run only after cancelling its jobs (steps 0–1 above), with
`bash "$MAIN/.claude/scripts/remove-worktree.sh" <worktree> [<branch>]` (`MAIN` = the main
checkout; it refuses while a live job or task process is in the worktree, stops its brokers,
then removes it), then run
`.claude/scripts/sweep-codex-orphans.sh` (stops this repo's orphaned brokers; reports suspected
orphan tasks without stopping them):
`implementation/orchestrator.md` → Role 1 → "Removing a worktree".

Resolve `COMPANION` as in `coder-codex.md` Step 2:

```bash
CODEX_ROOT="$(ls -d ~/.claude/plugins/cache/openai-codex/codex/*/ 2>/dev/null | sort -V | tail -1)"
COMPANION="$CODEX_ROOT/scripts/codex-companion.mjs"
```

### Brief hygiene

The forwarder parses flag-like tokens anywhere in the brief text, not only at the end. A
literal `--resume` quoted from omp's CLI made it look for an earlier Codex thread and fail;
`--fresh` and `--resume-last` are its own flags too. When a brief has to mention an omp or
other CLI flag, write it in prose ("the resume flag") or inside a fenced code block the
forwarder won't parse, and put `--effort high` (or any routing flag) only where you mean it.

The codex-companion app-server has an inactivity watchdog of about 180 s: a run that streams
nothing for that long ends with "Codex app-server produced no activity for 180s … Treating as
stalled" and exit 1. Long, high-effort final answers (a big plan after many file reads) trip
it. Ask for a COMPACT result (about 120 lines at most) and, when the context is already local,
no web search; that avoided the stall. After two stalls on the same task, stop retrying:
degrade to the Claude engine alone and say so in the report. The Codex Implementer (when
routing names it) stalls the same way when it reads a very long file in full before editing; small fixes go to it as line-range
briefs, and it reports each failed attempt and stops after two (`coder-codex.md` → Steps 3
and 4).

### Capacity and rate errors

A call can fail with "Selected model is at capacity", a 429 or another rate-limit error. That
is transient: retry the same call once after a short wait (about 60 s; `sleep 60` in Bash).
If the retry fails too, record Codex as unavailable for that step and go on with the
single-engine (Claude) verdict, saying in the report that Codex was unavailable and why.
Never drop the engine silently, and never report a dual-engine result when only one ran.
This applies to every Codex path: `codex:codex-rescue`, `codex review` and the Implementer's
`codex-companion.mjs` (where "go on" means STOP and report the blocker: the routed
Implementer writes product code, and a host never takes over).

### Example — single dispatch

```
Agent tool call:
  subagent_type: "codex:codex-rescue"
  description:    "Codex deep-dive on PCS-303 scroll bug"
  prompt:         "Investigate why the middle pane scrolls to the wrong target
                   after inserting a paged block in /apps/web/components/customer-flow/.
                   Reproduce, identify the root cause, propose a fix.
                   Repo: /Users/edevize/Developer/procesarecaietsarcini.
                   --effort high"
```

Return Codex's stdout to the user as-is. Do not paraphrase or summarize unless
asked.

## Review use (review-implementation)

The triple-engine review (`/review-implementation`, or `/implement` Role 2 when omp
is ready or risk is high) uses ONE Codex
engine — the native `codex review --base <BASE>` (or `--uncommitted`) reviewer — run by
the review orchestrator alongside the Claude comprehensive reviewer and omp (via glen), then
folded by the synthesizer. No N-pass voting, no shuffled diffs.

## Fallback (only if `codex:codex-rescue` is unavailable)

If the Agent tool reports "Unknown subagent" for `codex:codex-rescue`, use the plugin's
**native reviewer** for review/critique tasks — never `codex exec`:

```bash
codex review --base <BASE>                              # diff review vs a base branch
codex review --uncommitted "<adversarial instructions>" # critique uncommitted work / written material
```

For open-ended diagnosis with no agent path available, surface that to the user rather
than hand-rolling `codex exec` — keeping Codex on the plugin's authenticated runtime is
the doctrine (carries auth, config, and session management).

## Critical evaluation

Codex is a peer, not an authority. If its output contradicts something you know
to be true (recent library APIs, project conventions documented in `CLAUDE.md`),
push back to the user with evidence rather than deferring silently.
