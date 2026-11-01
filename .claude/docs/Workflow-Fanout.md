# Workflow fan-out

When to run harness work as one Workflow (many agents, one script) and how to keep it safe.

**Caps.** Read them from the routing source, never from this page:

```bash
bash .claude/scripts/routing.sh workflow_max_agents   # total agents in one run (50 at the time of writing)
bash .claude/scripts/routing.sh heavy_concurrency     # cargo-running or Codex agents at once (8)
```

`.claude/routing.json` holds the defaults; a confirmed owner override lives in
`.claude/auto-mode/routing.json`. If `routing.sh` exits non-zero, stop and report: do not guess a
cap.

## Use a workflow for

- **Audits:** the scanners, then one verifier per finding (two lenses, REPRODUCE and REFUTE, as in
  `.claude/agents/codebase/orchestrator.md` Step 3), then one synthesizer.
- **Audit fix waves:** one agent per fix group, each in its own worktree.
- **The review gate:** the engines in parallel, then one verifier per must-fix finding.
- **The AC source check:** one verifier per batch of about 5 ACs.
- **Harness batches:** one implementer and one reviewer per group.
- **Planning:** the explorers, the researchers and the two blind planners.
- **`/deep-debug` investigators.**

## Never fan out

- The fused Implementer loop: it needs one context from plan to green gate.
- The committer.
- Merges.
- Anything that writes the same file as another agent.

## Rules

- **Disjoint write scopes.** Every agent writes only in its own worktree or its own output file.
  Two agents that need the same file belong in one agent, or in sequence.
- **Heavy concurrency.** Agents that run cargo and agents that call Codex both count against
  `heavy_concurrency`. Read-only Claude agents count only against `workflow_max_agents`.
- **One test suite per target dir.** Each cargo agent works in its own worktree (own `target/`)
  or sets its own `CARGO_TARGET_DIR`; agents sharing a checkout run suites through
  `bash .claude/scripts/suite-lock.sh cargo test --locked`. Two suites in one target dir break
  each other (a concurrent `cargo build --example` re-links the binaries the other is running).
- **Briefs are files; committers re-read them.** A workflow agent's brief is a file path plus a
  section id, never only inline text: running workflow agents cannot receive SendMessage, so a
  revision reaches them only through the file, as a `> REVISED <ISO>: …` or
  `> REVOKED <ISO>: <decision> — <replacement>` line at the top of the affected section. Every
  agent that commits or merges records `shasum -a 256 <brief file>` when it starts, re-hashes
  right before it commits or merges, and on a change applies those lines first (or stops and
  reports when one contradicts committed work). Group agents do not read this file: the brief
  itself carries that instruction, or names `coder-claude.md` → Step 3, item 3.
- **Codex roles.** A Codex arm takes only a role that `routing.json` gives Codex (for example
  `routing.sh review` lists `codex`). Implementation goes to the engine `routing.sh implementation`
  names.
- **Failures are collected.** One agent's failure or timeout is recorded in the results and does
  not abort the run. The orchestrator decides afterwards whether to re-run that arm.
- **Structured output.** Every agent returns a structured result (schema in the script): status,
  files written, findings or verdict, and evidence paths.
- **Results, not transcripts.** The orchestrator reads the structured results and the files they
  name. It never reads an agent's transcript to find out what happened.

## Script authoring

Load the `workflow-authoring` skill before writing the Workflow script. It covers the script API,
resume and the known gotchas.

## Budget

Before running, name in the plan:

- the agent count per phase and in total (within `workflow_max_agents`);
- how many run cargo or Codex at once (within `heavy_concurrency`);
- the number of Codex calls, so Codex quota is spent on purpose (for example one Codex review of
  a combined diff rather than one per group).

A run that would exceed a cap is split into phases, not squeezed.
