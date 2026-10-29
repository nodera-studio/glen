---
allowed-tools: Read, Bash, Agent
description: Cluster the agent-gap log and propose promoting recurring shapes into permanent agents
argument-hint: [--threshold N]
model: inherit
---

# /audit-agent-gaps

Periodic review of `.claude/orchestrator/gaps.jsonl` — the record of every one-off subagent
`/orchestrate` mode spawned because nothing in `.claude/agents/README.md` fit. Clusters
similar gaps; a recurring shape (default threshold: 3) is a candidate for becoming a real
agent.

## Flow

1. Dispatch `.claude/agents/orchestrator/gap-auditor.md` (pass the threshold if given in
   `$ARGUMENTS`, default 3).
2. It reads `gaps.jsonl`, clusters open entries, and reports back — proposed new agent(s)
   with the evidence (which gap entries, how many, sample task summaries), or "no cluster
   met the threshold" if none.
3. **It does not write anything.** If the user approves a proposal, dispatch it again (or
   continue the same agent) with an explicit go-ahead to land the agent file(s), wire
   `README.md`, and mark the source `gaps.jsonl` entries `promoted`.

$ARGUMENTS
