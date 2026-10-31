---
allowed-tools: Read, Agent, Bash, Grep, Glob
description: Lean dual-engine code review (1 Claude + 1 Codex, 2-vote) on the current diff — the /implement Step 13 gate, runnable standalone
argument-hint: [optional scope hint, e.g. "vs main" or a path]
model: inherit
---

Run the lean dual-engine code-review gate as a standalone pass — the same gate
`/implement` runs at Step 13, but on demand against the current working changes.
This is intentionally lean (the omp third engine runs in `/review-implementation` and
in `/implement`'s triple-engine block); for the
exhaustive multi-pass audit use `/review-implementation` instead.

- **Run it as a workflow** (`.claude/docs/Workflow-Fanout.md`): the two engines in parallel,
  then one fresh read-only verifier per must-fix finding, which REPRODUCEs it (re-traces the
  cited `file:line`, or runs the failing case) before the vote. A must-fix that does not
  reproduce drops to a note. The 2-vote merge in step 3 is unchanged.
- **Linux-only code waits for CI.** When the diff adds or changes `cfg(target_os = "linux")`,
  `bwrap`, `unshare`, `netns`, `renameat2`, `omp-sandboxed-linux.sh` or a Linux job in
  `.github/workflows/`, wait for the draft PR's Linux CI first (`check (ubuntu-latest)` and
  `linux-sandbox`; `gh pr checks <pr>`), and give both engines the run id and any failing
  logs. With no run, the Claude pass reports "Linux-only code unverified: CI run required",
  which blocks the gate.

Optional scope hint: $ARGUMENTS

Steps:

1. Establish the diff to review:
   - Default: uncommitted changes — `git diff --stat HEAD` then `git diff HEAD`.
   - If `$ARGUMENTS` names a base (e.g. "vs main") or a path, scope to that
     (`git diff <base>...HEAD` or limit to the path).
   - If there are no changes, say so and stop.

2. Dispatch BOTH passes in a single message (two Agent tool calls in one
   tool-use block, so they run concurrently):
   - **Claude pass** — `subagent_type` default, prompt: "Read
     `.claude/agents/implementation/code-review.md` and follow its
     instructions. Review these changes: {diff scope + changed file list}."
   - **Codex pass** — `subagent_type: "codex:codex-rescue"`, prompt:
     "Independent full-category code review (security, architecture, data flow,
     performance, accessibility, error handling, test coverage) of these
     changes: {diff scope + changed file list}. Conventions are in CLAUDE.md and
     `.claude/rules/*`. If the diff touches rendered output (`site/`, a TUI,
     terminal rendering), build and run it and look at the output before calling
     it correct: screenshot the built page (`npx playwright screenshot` against
     `npx astro preview --host 127.0.0.1 --port <a random free port of your own>`
     run in the background in `site/` and stopped afterwards, across the full
     length of any animation), or run the CLI under a pty (`script -q /dev/null sh
     -c 'stty cols 60; cargo run -- <args>'`); a code trace alone is not enough,
     and if you cannot run it say the UI is unverified. Against the real ~/.omp
     run only read verbs (list, show, tail); for anything else point
     PI_CODING_AGENT_DIR at tests/fixtures or a scratch dir under $TMPDIR, or use
     the fake omp via GLEN_OMP_BIN. Never send, steer, follow-up, abort or switch
     the model of a live omp session. Report severity + category + file:line + issue + impact
     - fix + confidence. Read-only — report only, do not fix. Do NOT write any
     file: return the full review as your final message text; the orchestrator
     saves it. Run in the foreground: wait for the Codex task to finish and return
     the full review as your final message, never only a background task id.
     --wait --effort high".
   - Never name an output path in the Codex brief: its sandbox is workspace-write,
     does not escalate, and writes only inside its own cwd (your shell's directory
     at dispatch, often a worktree), so a write elsewhere fails. Save the returned
     text yourself if it must be on disk.
   - The `--wait` is load-bearing: without it the forwarder may background a long
     review and return at once with a task id. If a task id comes back anyway,
     fetch the review (`codex-companion.mjs status <id> --wait`, then `result <id>`)
     before merging; never merge a pass you have not read.
   - If `codex:codex-rescue` is unavailable, fall back per
     `.claude/skills/codex/SKILL.md`; if Codex is unreachable, run the Claude
     pass alone and note the degraded (single-engine) gate.
   - If the Codex pass fails on capacity or rate ("Selected model is at
     capacity", 429, rate limit), retry it once after a short wait (about 60 s).
     If the retry fails too, record Codex as unavailable and merge the Claude pass
     alone as a single-engine verdict, saying so in the report. Never drop the
     engine silently.
   - If you save either pass to a file, name it `claude-review.md` /
     `codex-review.md`, never `claude.md`: on macOS's case-insensitive filesystem
     Claude Code loads any `claude.md` as a nested `CLAUDE.md`, so the review
     would be injected as project instructions. This holds for every way of
     creating the file, including shell redirects and `mv` (the guard hook covers
     the Write tool only).

3. Merge by 2-vote consensus and present ONE report:
   - Both engines raise it → **HIGH — must fix**.
   - CRITICAL from either engine → **must fix** (survives a single vote).
   - HIGH from one engine only → judgement call; list as a recommended fix.
   - MEDIUM/LOW single-engine → note only (do not block).
     Group by must-fix / recommended / notes, each with file:line and the
     suggested change.

This command does NOT modify code or commit — it reports. To apply fixes, act on
the report yourself or run `/implement` (whose Step 14 rework loop automates the
fix cycle).
