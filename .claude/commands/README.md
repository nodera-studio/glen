# Commands

25 slash commands, each a thin dispatcher into the agent suite under `agents/`. Grouped
here by workflow — see `agents/README.md` for the full agent-suite index each command
drives.

## Plan → build → ship

| Command | Dispatches | What it does |
| --- | --- | --- |
| `/plan-implementation` | `agents/plan/orchestrator.md` | Explore, research, dual-engine architect + judge, dual-engine plan + judge — see `agents/plan/README.md` |
| `/implement` | `agents/implementation/orchestrator.md` | Plan → routed Implementer (`routing.sh implementation`, Claude by default) → triple-engine review → docs + conformance tail → re-index — see `agents/implementation/README.md` |
| `/implement-quick` | `agents/implementation/orchestrator-quick.md` | Same shape, fewer gates, for small/low-risk changes |
| `/review-implementation` | `agents/review-implementation/orchestrator.md` | Standalone Claude ‖ Codex ‖ omp review + synthesizer on the current branch |
| `/code-review` | `agents/implementation/code-review.md` | Lean 2-engine version of the review gate, runnable standalone |
| `/respond-to-review` | `agents/respond-to-review.md` | Classify PR review comments, verify claims, draft responses, apply fixes |
| `/run-ci` | `agents/local-ci.md` | Local CI with cargo, mirroring `ci.yml` (fmt check, clippy `-D warnings`, check, build, test) |

## Autonomous / unattended

| Command | Dispatches | What it does |
| --- | --- | --- |
| `/goal` | (writes `.claude/auto-mode/state.md`) | Declare or update the auto-mode objective (and whether it ends in a release tag) — intent only, doesn't start the loop |
| `/auto-mode` | `skills/auto-mode/SKILL.md` | Drive a declared goal to merged completion, wave by wave, with no user gate in between — see the diagram below |

## Explore, research, brainstorm

| Command | Dispatches | What it does |
| --- | --- | --- |
| `/explore` | `agents/plan/explorer.md` | Deep read-only codebase exploration — map code relevant to a task |
| `/research` | `agents/research/agent.md` (fan-out) | Exa multi-agent fan-out on any topic — see the diagram below |
| `/brainstorm` | `agents/brainstorm.md` | Turn a vague idea into a requirements doc, with optional browser mockups |
| `/bootstrap-project` | `agents/bootstrap/orchestrator.md` | Onboard this harness onto a new project — see `agents/bootstrap/README.md` |

## Debug

| Command | Dispatches | What it does |
| --- | --- | --- |
| `/deep-debug` | `agents/debug/{investigator,queue}.md` + `agents/debug/adversary.md` | Claude ‖ Codex investigator (+ optional omp), then mandatory adversarial verification — see `agents/debug/README.md` |

## Codebase-wide audits

| Command | Dispatches | What it does |
| --- | --- | --- |
| `/codebase-audit` | `agents/codebase/orchestrator.md` | Tier 1 quick health check (default) or `deep` for all tier 2 audits |
| `/security-audit` | `agents/codebase/security-audit/orchestrator.md` | Cartographer → deep agents → evidence-demanding synthesizer, ASVS 5.0 L2 |
| `/error-audit` | `agents/codebase/error-audit/orchestrator.md` | 2 agents (Claude ‖ Codex) over the crate's error paths |
| `/docs-audit` | `agents/codebase/docs-audit/orchestrator.md` | 3 agents (2 Claude + 1 Codex) — docs vs. actual code |
| `/tech-debt-audit` | `agents/codebase/tech-debt-audit/orchestrator.md` | 4 agents (3 Claude + 1 Codex) — patterns, complexity, TODOs, library currency |
| `/security-scan` | (inline — AgentShield + mcp-scan) | Scan `.claude/` config/hooks/MCP setup for security drift |

## Codex / cross-model

| Command | Dispatches | What it does |
| --- | --- | --- |
| `/codex` | `codex:codex-rescue` (plugin agent) | Dispatch a task to OpenAI Codex |
| `/codex-advisor` | `codex:codex-rescue` (read-only) | Ask Codex for an adversarial second opinion, no edits |

## Orchestrator mode

| Command | Dispatches | What it does |
| --- | --- | --- |
| `/orchestrate` | `agents/orchestrator/orchestrator-mode.md` | Toggle dispatch-only mode — match an existing agent or spawn + log a scoped one-off — see `agents/orchestrator/README.md` |
| `/audit-agent-gaps` | `agents/orchestrator/gap-auditor.md` | Cluster the gap log, propose promoting recurring shapes into permanent agents — report-then-approve |

## Meta

| Command | Dispatches | What it does |
| --- | --- | --- |
| `/improve-prompt` | (inline — `prompts/prompt-engineering-*.md`) | Optimize a prompt or agent file against documented best practices |

---

## `/auto-mode` — the unattended wave loop

The most complex command in the suite: it drives `/plan-implementation` and
`/implement` in a loop against a declared goal, with **no user gate between waves**,
until the whole goal is merged on `main` (and tagged as a release, if that's what the goal
asked for).

```mermaid
flowchart TD
    G["/goal &lt;objective&gt;"] --> ST[".claude/auto-mode/state.md<br/>(shared state — survives compaction)"]
    ST --> AM["/auto-mode"]

    AM --> RESOLVE{{"Resolve waves:<br/>Glen board child issues first,<br/>plan-doc fallback,<br/>else run /plan-implementation"}}

    RESOLVE --> WAVE

    subgraph WAVE["Per-wave pipeline — repeats with NO user gate between waves"]
        direction TB
        W1["Bootstrap: branch off main,<br/>board issue → in_progress"]
        W2["/plan-implementation (skip if a plan exists)"]
        W3["/implement (includes the Claude ‖ Codex /code-review gate)"]
        W4["Pre-push cargo checks → open PR into main,<br/>board issue → in_review"]
        W5["Poll CI (fmt, clippy, test on Linux + macOS)<br/>+ PR comments; triage + fix every finding"]
        W6["CI green + no unresolved comments →<br/>memory_store referencing the PR → squash-merge"]
        W7["Close wave: issue → done + shipped:main,<br/>delete branch"]
        W1 --> W2 --> W3 --> W4 --> W5 --> W6 --> W7
    end

    WAVE -->|"more waves"| WAVE
    WAVE -->|"all waves done"| DONE{{"Goal asks for a release?"}}
    DONE -->|no| FIN1["done — merged on main"]
    DONE -->|yes| REL["release skill: version-bump PR,<br/>annotated v0.x.y tag on main,<br/>issues tagged released:v0.x.y"]
    REL --> FIN2["done — released"]
```

Escalates to the user only for genuine human-required blockers (credentials, material
scope trade-offs, external approvals, a wave red 3+ cycles with no path forward) — never
just to ask "does this look right?" between waves. See `skills/auto-mode/SKILL.md` and
its `references/loop-procedures.md` for the full state-file template and escalation ladder.

## `/research` — Exa multi-agent fan-out

```mermaid
flowchart LR
    Q["/research &lt;topic&gt;<br/>--quick | --deep | (default)"] --> DEC["Decompose into<br/>3-5 independent sub-questions"]
    DEC --> FAN

    subgraph FAN["Fan out — parallel, token-isolated"]
        direction TB
        S1["Sub-agent 1<br/>Exa + Context7"]
        S2["Sub-agent 2<br/>Exa + Context7"]
        S3["Sub-agent N<br/>(1 for --quick, up to 5 for --deep)"]
    end

    FAN --> MERGE["Merge + dedupe + reconcile contradictions"]
    MERGE --> CITE["Citation pass (15-30 sources)"]
    CITE --> SAVE[".claude/research/{date}-{slug}.md"]
```
