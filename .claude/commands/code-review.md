---
allowed-tools: Read, Agent, Bash, Grep, Glob
description: Lean dual-engine code review (1 Claude + 1 Codex, 2-vote) on the current diff — the /implement Step 13 gate, runnable standalone
argument-hint: [optional scope hint, e.g. "vs main" or a path]
model: inherit
---

Run the lean dual-engine code-review gate as a standalone pass — the same gate
`/implement` runs at Step 13, but on demand against the current working changes.
This is intentionally lean (BugBot + CodeRabbit cover the PR); for the
exhaustive multi-pass audit use `/review-implementation` instead.

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
     `npm run preview` in `site/`, across the full length of any animation), or
     run the CLI under a pty (`script -q /dev/null sh -c 'stty cols 60; cargo run
     -- <args>'`); a code trace alone is not enough, and if you cannot run it say
     the UI is unverified. Report severity + category + file:line + issue + impact
     - fix + confidence. Read-only — report only, do not fix. --effort high".
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
     would be injected as project instructions.

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
