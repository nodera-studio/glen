# Orchestrator Mode

Turns the main session into a pure dispatcher for arbitrary tasks: decompose, match an
existing agent from `agents/README.md` and dispatch it, or — only when nothing fits —
spawn a scoped one-off subagent and log why. The log itself becomes the input to a second,
periodic pass that decides whether a recurring one-off shape has earned a permanent place
in the suite.

## Dispatch loop

```mermaid
flowchart TD
    O["/orchestrate"] --> MODE["Marker file + orchestrator-mode.md<br/>loaded as standing instructions"]
    MODE --> TASK["User gives a task"]
    TASK --> MATCH{{"Matches an existing<br/>agent/command/skill?"}}

    MATCH -->|"yes — common case"| DISPATCH["Dispatch it normally<br/>no gap logged"]
    MATCH -->|"no"| PICK["Pick closest fallback type<br/>(general-purpose, Explore, ...)<br/>write a tightly scoped one-off prompt"]
    PICK --> SPAWN["Dispatch the one-off"]
    SPAWN --> LOG["record-gap.mjs --json '{...}'<br/>BEFORE reporting back to the user"]
    LOG --> GAPS[(".claude/orchestrator/gaps.jsonl<br/>+ GapLog.md")]

    DISPATCH --> REPORT["Report result to user"]
    LOG --> REPORT
```

The main loop never calls Edit/Write/NotebookEdit directly in this mode — every change
goes through a dispatched agent, matched or one-off. There's no hook backing this; see
"Why prompt-level, not a hook" below.

## Gap-cluster-and-promote loop (`/audit-agent-gaps`)

```mermaid
flowchart TD
    A["/audit-agent-gaps [--threshold N]"] --> READ["Read gaps.jsonl<br/>status: open only"]
    READ --> CLUSTER["Cluster semantically —<br/>same underlying need, not just shared tags"]
    CLUSTER --> THRESH{{"Cluster size >= threshold?<br/>(default 3)"}}

    THRESH -->|"below"| WATCH["Report as 'watching' —<br/>no proposal yet"]
    THRESH -->|"meets"| DRAFT["Draft: name, mission, model/effort tier,<br/>prompt skeleton, where it slots in,<br/>evidence (source gap IDs)"]
    DRAFT --> REPORT["Report only — writes nothing"]

    REPORT -.->|"user approves a named proposal"| LAND["Write agent file(s)<br/>update README.md + commands/<br/>mark gaps.jsonl entries 'promoted'<br/>move rows to GapLog.md's Promoted table"]
```

## Why prompt-level, not a hook

A `PreToolUse(Edit|Write|NotebookEdit)` hard block gated on the mode marker was considered
and rejected: hooks configured in `settings.json` fire for every tool call in the session,
including calls made by the very subagents this mode dispatches to do the writing — there's
no reliable signal distinguishing "the top-level loop called this" from "a dispatched
subagent called this." A hard block would therefore also block legitimate subagent writes,
defeating the point. Enforcement is the same prompt-discipline mechanism the rest of the
suite already relies on for "orchestrators never take autonomous git/external actions" —
no new hook, no `settings.json` change, no collateral-block risk.

## Files in this folder

| File | Role |
| --- | --- |
| `orchestrator-mode.md` | Governing prompt loaded by `/orchestrate` — match-first, one-off-and-log fallback, never edit directly |
| `gap-auditor.md` | Dispatched by `/audit-agent-gaps` — clusters `gaps.jsonl`, proposes promotions, writes only on explicit approval |

## Entry points

`/orchestrate` (toggle) and `/audit-agent-gaps` (periodic review) — see
`commands/orchestrate.md` and `commands/audit-agent-gaps.md`.

## What it does NOT do

Gap logging only fires for genuine capability gaps — a task whose *kind* of work has no
harness home — not for "this particular one-off is small." A tiny fix still gets matched
normally if a fitting agent exists; it's the absence of a matching *shape*, not the size of
the task, that gets recorded. And `/audit-agent-gaps` never writes a new agent file itself
without an explicit go-ahead — the promotion bar is evidence (a recurring cluster) plus a
human decision, same as the optional Mnemosyne learned-rules loop's promotion gate.
