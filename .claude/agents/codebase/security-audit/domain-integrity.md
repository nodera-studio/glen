---
name: security-audit-domain-integrity
description: "Stage 2 (project-specific slot) — this project's own business-logic / data-integrity audit agent (e.g. financial calculations, tenant isolation, state-machine correctness). Copy the shape of access-control-idor.md and fill in your domain's invariants."
model: inherit
effort: xhigh
color: red
---

# Domain Integrity (fill in for THIS project)

<role>
This file is a STUB TEMPLATE, not a ready-to-run agent. In the source project this
suite was ported from, this slot held two domain-specific agents: one auditing
database-level multi-tenant row isolation, and one auditing the source project's own
financial-calculation correctness. Neither generalizes — they were entirely
domain-specific. What DOES generalize is the *shape*: a Stage-2 deep agent that audits
the ONE OR TWO business-invariant classes this project cares about most, working from
the cartographer's map, reporting candidate findings for the synthesizer to adjudicate.
</role>

**Done when:** you have replaced this file's content with a real agent that audits
this project's actual domain invariants — modeled on `access-control-idor.md`'s
structure (role → what to check → engines → report-only, no fixes).

## How to fill this in

1. **Identify this project's highest-value domain invariant(s).** Ask: what class of
   bug would be a business-critical failure if a scanner and a generic code reviewer
   both missed it? Examples from other domains:
   - Multi-tenant SaaS: row-level tenant isolation (who can read/write whose data).
   - Fintech / billing: money math correctness under adversarial input (rounding,
     currency, tampering, double-charging).
   - Regulated documents: a state machine with legally-binding transitions (no
     edit-after-finalize, no back-dating, gap-less sequential IDs).
   - Marketplaces: inventory/balance consistency under concurrency (no
     double-allocation, no lost updates).
   - Healthcare / safety-critical: dosage or safety-interlock invariants.
   Pick 1-2 — this is a Stage-2 DEEP agent, not a general-purpose one.

2. **Decide single- vs. dual-engine** per the orchestrator's rule of thumb: if the
   invariant is anchored to a deterministic test you can generate and run (the gold
   standard — e.g. a property test, a database-level isolation test), single-engine
   suffices because the test is the ground truth. If it's open-ended reasoning across
   files with no deterministic backstop, dual-engine (Claude ‖ Codex) adds real recall.

3. **Write the agent** in the shape of `access-control-idor.md`:
   - `<role>` — what this agent audits and why it's a distinct Stage-2 agent.
   - **What to check** — a concrete, enumerable list of the invariant's failure modes
     for THIS project's actual code (not a generic checklist — name real files/patterns).
   - **Engines** — which engine(s), and how findings are handed to the synthesizer.
   - Report-only. Never fix code. Never skip evidence.

4. **Wire it into the orchestrator.** In `security-audit/orchestrator.md`'s Stage 2,
   replace the single dispatch line below with your new agent (or two, if you kept a
   two-invariant split):

   ```
   Agent tool:
     prompt: "Read .claude/agents/codebase/security-audit/domain-integrity.md and follow it
              (once you've replaced its content with this project's real agent).
              Work from the cartographer's map. Report candidate findings only."
   ```

5. **Update the cartographer** (`audit-cartographer.md`) to map the surfaces your new
   agent needs — e.g. add a "the invariant's state machine" or "the money-calc sites"
   section to what it inventories, mirroring how it already maps auth/tenant/PII surfaces.

## If this project has no single dominant business invariant

Some projects (a CLI tool, a static site generator, a library with no persistent state)
genuinely don't have a domain-integrity class worth a dedicated Stage-2 agent. In that
case, delete this file and remove its dispatch from `security-audit/orchestrator.md`'s
Stage 2 — the audit still runs with `access-control-idor.md` alone. Don't force a
domain-integrity agent where the domain has no integrity invariant to speak of.
