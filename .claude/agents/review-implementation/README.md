# Review Implementation Workflow (Triple-Engine + Synthesizer)

Review your own implementation before creating a PR. Three independent review engines
for recall, one synthesizer for precision. Branch-aware. Used standalone via
`/review-implementation` and inside `/implement` as Role 2 — identical machinery.

```mermaid
flowchart TD
    O["Orchestrator<br/>detects base branch, gathers the<br/>working-tree diff (incl. uncommitted)"] --> ENGINES

    subgraph ENGINES["Three engines review the SAME diff in parallel — blind to each other"]
        direction LR
        E1["Claude<br/>comprehensive-review.md<br/>all 6 domains, one pass"]
        E2["Codex<br/>codex review --base<br/>(never codex exec)"]
        E3["CodeRabbit<br/>coderabbit review<br/>--plain --agent"]
    end

    ENGINES --> SYN["Synthesizer (4th Claude agent)<br/>dedupe by defect · cross-engine agreement = confidence ·<br/>NEW/MODIFIED/PRE-EXISTING provenance vs merge-base ·<br/>ordered fix plan · memory decision"]

    SYN --> OUT["Unified report:<br/>must-fix · notes · pre-existing ·<br/>fix plan · recommendation<br/>(BLOCK / REVIEW_REQUIRED / APPROVE_WITH_NOTES / APPROVE)"]
```

Total: 4 agents (3 engines + synthesizer) — no multi-pass voting, no shuffled diffs.

## Branch awareness

Reviews authored working-tree changes against the merge-base (default `staging`,
including uncommitted + untracked files). For branches with merge commits, the engines
see the full diff; the synthesizer flags any merge that dropped or duplicated work.

## Entry point

`/review-implementation` (standalone), or `/implement` Role 2 (in-pipeline), or read
`orchestrator.md` directly.

## Output contract

A unified report: must-fix (NEW+MODIFIED, ranked, engine-attributed) · notes
(single-engine MEDIUM/LOW) · pre-existing (informational) · an ordered fix plan · a
memory decision · and a recommendation (BLOCK / REVIEW_REQUIRED / APPROVE_WITH_NOTES /
APPROVE).
