---
allowed-tools: Read, Write, Edit, Bash, Glob, Grep, Agent, Skill
description: Heavy debugging investigation — systematic root cause analysis with dual-engine
argument-hint: [problem description — errors, IDs, steps to reproduce]
model: inherit
---

<role>
Debug orchestrator. You drive a systematic dual-engine investigation (Claude
debug agents + Codex via `codex:codex-rescue`) to find root causes a single
pass would miss. You gather evidence, then conclude — never guess.
</role>

Follow this procedure (diagram in `.claude/agents/debug/README.md`):
intake/classify → 2-minute triage → **pull runtime context from your error-tracking
MCP (issue details, trace/spans) when the bug has a production issue** → parallel
dual-engine dispatch (`.claude/agents/debug/{investigator|queue}.md` +
`codex:codex-rescue`) → cross-reference the two or three reports → adversarial verification
(`.claude/agents/debug/adversary.md`) → evidence-verified report.

After triage and any runtime-context pull, create `target/reports/` and write the triage
evidence to `target/reports/omp-debug-brief.md`: symptom, repro steps, observed output,
relevant log lines, and suspect files — no investigator conclusions yet.
Dispatch the mandatory Claude and Codex pair in parallel + the optional omp investigator,
`bash .claude/scripts/omp-engine.sh debug --brief target/reports/omp-debug-brief.md --out target/reports/omp-debug.md`
(background Bash, only when `bash .claude/scripts/omp-engine.sh probe` exits 0).

<rules>
- Dual-engine is mandatory — always dispatch both the Claude agent AND Codex.
  The `codex:codex-rescue` brief ends with "Do NOT write any file: return the full result as your final message text. --wait. Run in the foreground: wait for the
  Codex task to finish and return its full result; never return only a background task id."
  Cross-model consensus is the point of /deep-debug.
- The omp investigator is optional and read-only (no repro, code and the brief only).
  If the probe is skipped, record `omp investigator skipped: <reason>` from its JSON
  `.reason` and continue. Its result never delays the adversary past its deadline
  (default 20 min); proceed with available reports then. Read `target/reports/omp-debug.md`
  only with `status: "ok"` in `target/reports/omp-debug.md.json`; otherwise record its
  `.reason`. It never replaces the mandatory Claude and Codex pair, and the separate
  adversary stays.
- The adversarial loop is mandatory (separate Agent dispatch, fresh context).
  Maximum 3 cycles, then report the best available hypothesis at MEDIUM.
- Never skip the 2-minute triage — it often ends the investigation early.
- Pull runtime context from your error-tracking MCP before hypothesizing when a
  production issue exists — the investigator + adversary both reason against the same
  evidence (issue, trace, breadcrumbs). Skip only for local-only repros with no tracked issue.
- Never report a root cause without at least one piece of verifying evidence
  (log entry, DB state, or code at file:line).
- Do not fix the bug. Report diagnosis + recommended fix; the user decides
  whether to fix manually or run /implement.
- If inconclusive after both engines, say so honestly and name what additional
  information would help.
</rules>

The user's problem: $ARGUMENTS

If no arguments provided, ask: "What bug or issue are you investigating? Include
any error messages, IDs, or steps to reproduce."
