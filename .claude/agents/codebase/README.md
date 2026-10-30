# Codebase Analysis Agents

Two-tier audit architecture for full-codebase analysis. These scan the **entire codebase**, not just diffs or individual PRs.

```mermaid
flowchart TD
    CMD["/codebase-audit"] --> T1["Tier 1 — Quick health check (default)<br/>4 lightweight scans, &lt;5 min:<br/>dependency · tech-debt (light) ·<br/>CLI surface (light) · docs freshness (light)"]
    CMD -->|"deep"| T2

    subgraph T2["Tier 2 — Deep audits (each independently invocable)"]
        direction LR
        SEC["/security-audit<br/>up to 6 agents"]
        ERR["/error-audit<br/>2 agents (1+1)"]
        DOC["/docs-audit<br/>3 agents (2+1)"]
        DEBT["/tech-debt-audit<br/>4 agents (3+1)"]
    end
```

**Zoom: `/security-audit`'s internal shape** (the deepest of the five — cartographer →
focused deep agents → evidence-demanding synthesizer, not just N parallel scanners):

```mermaid
flowchart LR
    MAP["Stage 1 — Cartographer<br/>(1 Claude agent)<br/>builds the shared app-intent map:<br/>endpoints+authz, tenant key,<br/>financial sites, PII/SSRF surface"] --> DEEP

    subgraph DEEP["Stage 2 — Deep agents (share the cartographer's map)"]
        direction TB
        IDOR["access-control-idor.md<br/>dual-engine (Claude ‖ Codex)<br/>whole-app authz, IDOR, exposure"]
        DOM["domain-integrity.md<br/>this project's own business-invariant<br/>audit (fill in per-project)"]
    end

    DEEP --> SYN["Stage 3 — Synthesizer<br/>demands evidence (exploit path / call<br/>chain / failing test) before a finding<br/>survives; maps to ASVS 5.0 L2"]
    SYN --> REP[".claude/reviews/security-audit-{date}.md"]
```

Why this shape and not 5 identical scanning passes: LLM security scans plateau at 2-3
passes (more adds false positives, not findings) and hit >50% false-positive rates
without evidence-demanding synthesis — see `security-audit/orchestrator.md`'s "Why this
shape" section for the full reasoning.

## Tier 1: Quick Health Check

Dispatched by `/codebase-audit` (default mode). Finishes in under 5 minutes.

| Agent | File | What it scans |
|-------|------|---------------|
| Dependency Audit | `dependency-audit.md` | RustSec advisories, outdated crates, licenses, toolchain currency |
| Tech Debt (light) | _(inline in orchestrator)_ | TODO/FIXME counts, panics in non-test code, large files, lint escapes |
| CLI Surface (light) | _(inline in orchestrator)_ | Subcommands, flags, `--json` coverage, `--help` renders |
| Docs Freshness (light) | _(inline in orchestrator)_ | Doc commit dates vs. recent `src/` changes |

Dead code is clippy's job (`dead_code`, `unused_*` lints fail CI under `-D warnings`), so
there is no LLM pass for it.

## Tier 2: Deep Audits

Each is a standalone multi-agent audit with its own orchestrator. glen has no HTTP API, so the upstream API audit was removed. Dispatched individually or all at once via `/codebase-audit deep`.

| Audit | Command | Directory | Agents | Engine |
|-------|---------|-----------|--------|--------|
| Security | `/security-audit` | `security-audit/` | up to 6 (cartographer → IDOR/domain-integrity → synthesizer) | Union + evidence-synth |
| Error Paths | `/error-audit` | `error-audit/` | 2 (1 Claude + 1 Codex) | Dual-engine |
| Documentation | `/docs-audit` | `docs-audit/` | 3 (2 Claude + 1 Codex) | Dual-engine |
| Tech Debt | `/tech-debt-audit` | `tech-debt-audit/` | 4 (3 Claude + 1 Codex) | Dual-engine |

## Standalone (Not Part of Audits)

| Agent | File | Purpose |
|-------|------|---------|
| Docs Expert | `docs-expert.md` | Documentation Q&A -- answers questions, not an audit |

## When to Run

| Audit | Frequency | Trigger |
|-------|-----------|---------|
| Quick health check | Weekly | After major merges, routine check-ins |
| Security audit | Quarterly | Pre-release, after auth/security changes |
| Error audit | Per feature | After major features, before error handling work |
| Docs audit | Quarterly | After major features, pre-release |
| Tech debt audit | Quarterly | Sprint planning, debt paydown sprints |

## Invocation

```bash
# Quick health check (default)
/codebase-audit

# All tier 2 deep audits
/codebase-audit deep

# Individual deep audits (standalone, no dependency on /codebase-audit)
/security-audit
/error-audit
/docs-audit
/tech-debt-audit

# On-demand Q&A (not an audit)
# Invoke docs-expert agent directly
```

## Research-Backed Improvements

The tier 2 audits incorporate several techniques from prompt engineering research:

- **Semi-formal reasoning:** Each agent uses a structured template (premise, execution trace, discrepancy, severity) to reduce false positives
- **Mid-thought abort:** Agents stop analyzing concerns that turn out to be handled correctly, saving context for real issues
- **Conventions-first anchoring:** Agents read project conventions (CLAUDE.md, rules/) before scanning code, reducing false positives from unfamiliar patterns
- **Evidence-demanding synthesis (security):** the synthesizer confirms each finding against an exploit path / call chain / failing test before it ships — union for recall, evidence for precision (replaces the old checklist-perturbation passes)
- **Dual-engine consensus:** Claude and Codex agents scan independently; findings confirmed by both engines get HIGH confidence
