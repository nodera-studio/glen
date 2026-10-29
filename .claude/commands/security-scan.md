---
allowed-tools: Read, Bash, Grep, Glob
description: Scan .claude/ config, hooks, and MCP setup for security drift (AgentShield static + advisory mcp-scan)
argument-hint: [low|medium|high|critical]
model: inherit
---

Run the config-security scan — the compensating control for the widened
autonomy surface (the `.env`/`secrets` read-denies were intentionally lifted;
this scanner WATCHES that surface, it does NOT re-lock it). See
`.claude/docs/config-security.md` for the residual-risk note.

Steps:

1. Run `bash scripts/security-scan.sh ${ARGUMENTS:-medium}` from the repo root.
   This invokes AgentShield (`ecc-agentshield scan`, static config + hooks + MCP +
   CLAUDE.md → JSON report at `reports/agentshield.json` + a terminal report), runs
   the advisory regression gate vs `.agentshield/baseline.json`, and, if `uvx`/`uv`
   is present and MCP servers are reachable, Invariant `mcp-scan --local-only` for
   tool-pinning drift (advisory).
2. Summarize the output: counts per severity, then each finding's `file:line` with
   a one-line fix. Group accepted-risk items (see the residual-risk note) separately
   from genuine new findings.
3. Do NOT auto-apply `--fix`, and do NOT modify `.claude/settings.local.json`
   (`enableAllProjectMcpServers`) or re-add any `permissions.deny` for
   `.env`/secrets/MCP — those are deliberate full-autonomy choices. Flag them as
   accepted residual risk; the operator decides any change. Report only.

Default min-severity is `medium` (local visibility, non-blocking). The CI gate
(`.github/workflows/config-security.yml`) is the blocking layer: it fails only on
a NEW finding vs the committed baseline (`.agentshield/baseline.json`, AgentShield
`--gate` exit 3), so existing accepted-risk findings do not block — only drift does.

User's argument: $ARGUMENTS
