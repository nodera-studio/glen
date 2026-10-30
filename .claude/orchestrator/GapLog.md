# Agent Gap Log

Every time `/orchestrate` mode dispatches a one-off subagent because nothing in
`.claude/agents/README.md` fits the task, `.claude/scripts/orchestrator/record-gap.mjs`
appends a row here (and a matching JSON line to `gaps.jsonl`, which is what
`/audit-agent-gaps` actually reads — this file is the human-skimmable index).

A single gap is just a data point, not a verdict. `/audit-agent-gaps` periodically clusters
the open gaps below; a recurring shape (3+ similar entries by default) becomes a proposed
permanent agent, reported for approval before anything is written. See
`.claude/agents/orchestrator/gap-auditor.md`.

Do not hand-edit the tables below — use `record-gap.mjs` (append) or `/audit-agent-gaps`
(promote). This file is meant to be checked in and accumulate as project history, same as
`.claude/plans/` and `.claude/tech-debt/`.

## Open gaps

| ID | Date | Task | Closest agent | Why no fit | Ad-hoc agent | Tags |
| --- | --- | --- | --- | --- | --- | --- |
| g-mvucl7jw-fyv7 | 2026-10-30 | Fix logged harness friction in .claude/ (agent prompts, orchestrators, skills) in parallel with product waves — the harness self-improvement loop | .claude/agents/orchestrator/gap-auditor.md | gap-auditor only clusters gaps and proposes new agents; nothing edits harness prompts from the friction log | general-purpose | harness, self-improvement, prompt-maintenance |
| g-pending-codex-handoff | 2026-10-30 | Hand a Codex planner/architect report (returned only as the codex:codex-rescue final message) to the plan judge-selector | .claude/agents/plan/judge-selector.md | the plan pipeline has no step that persists the Codex pass; the orchestrator had to jq-extract the transcript into .claude/plans/<slug>.codex.md | none (orchestrator shell step) | plan, codex, handoff |
| g-mvudr24v-k0je | 2026-10-30 | Run network-dependent setup (npm scaffold/install, morphin CLI add, playwright browsers) that the Codex Implementer sandbox cannot do | .claude/agents/implementation/coder-codex.md | the Implementer runs in a Codex sandbox that may lack network; nothing in /implement owns a network pre-flight, so the orchestrator ran it by hand | none (orchestrator shell step) | implement, network, preflight, site |
| g-mvudr26q-dks1 | 2026-10-30 | Second harness self-improvement batch (fix logged friction in .claude/ prompts and skills) | .claude/agents/orchestrator/gap-auditor.md | recurrence of g-…harness-fixer gap: still no agent that edits harness prompts from the friction log | general-purpose | harness, self-improvement, prompt-maintenance, recurrence |
| g-mvufa9r7-xsd4 | 2026-10-30 | anonymise candidate plans for a blind judge | .claude/agents/plan/judge-selector.md | no step anonymises candidates: the judge saw engine-named files (arch-claude/arch-codex) and labels, so it was not blind; now an orchestrator step (Step 4/4.5 and Step 6 of plan/orchestrator.md) coin-flips the order, strips authorship tells, and keeps the mapping private | none (orchestrator step) | plan, judge, blind-selection, harness |

## Promoted

| ID | Date | Task | Promoted to |
| --- | --- | --- | --- |
