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
     `.claude/rules/*`. Report severity + category + file:line + issue + impact
     - fix + confidence. Read-only — report only, do not fix. --effort high".
   - If `codex:codex-rescue` is unavailable, fall back per
     `.claude/skills/codex/SKILL.md`; if Codex is unreachable, run the Claude
     pass alone and note the degraded (single-engine) gate.

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
