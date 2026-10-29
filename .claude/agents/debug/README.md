# Debug Workflow

Heavy investigation workflow for systematic root cause analysis. Not for everyday debugging -- use when you are stuck and need a methodical, exhaustive investigation.

```mermaid
flowchart TD
    B["/deep-debug &lt;bug description&gt;"] --> T["2-minute triage<br/>(often ends the investigation early)"]
    T -.->|"production issue exists"| SEN["Pull Sentry runtime context<br/>(issue, trace/spans, Seer)"]
    T --> ENGINES
    SEN --> ENGINES

    subgraph ENGINES["Mandatory dual-engine dispatch — parallel, independent"]
        direction LR
        C["Claude subagent<br/>investigator.md or queue.md"]
        X["Codex agent<br/>codex:codex-rescue"]
    end

    ENGINES --> M["Cross-reference the two reports"]
    M --> ADV["Mandatory adversarial verification<br/>(adversary.md, fresh context,<br/>max 3 cycles) — tries to DISPROVE<br/>the leading hypothesis"]
    ADV --> RC["Evidence-verified report<br/>(never fixes the bug — you decide<br/>whether to fix manually or run /implement)"]
```

The adversarial step is the point: a hypothesis that survives someone actively trying to
disprove it (checking it against real code and runtime evidence, not just plausibility)
is worth acting on. One that doesn't survive sends you back to investigate, cheaper than
shipping a wrong fix.

## Specialists

| Agent | File | Focus |
|-------|------|-------|
| `debug-investigator` | `investigator.md` | General application errors -- request flows, stack traces, logs, state |
| `debug-queue` | `queue.md` | Background job / async pipeline failures -- stalled jobs, stuck processing, worker issues |
| `debug-adversary` | `adversary.md` | Mandatory hypothesis-disproving pass before any root cause is reported |

## Entry Point

Invoke via `/deep-debug` slash command. Dual-engine dispatch is mandatory — cross-model
consensus is the point of this workflow, not an optional extra.

## When to Use

- A bug persists after initial investigation
- Processing is stuck with no obvious cause
- Error messages are misleading or absent
- Multiple systems interact and the failure point is unclear

## When NOT to Use

- Simple bugs with clear stack traces -- just fix them
- Configuration issues -- check `.env` files and Docker logs directly
- Known issues documented in `docs/` -- read the docs first
