# Config-security scanner — residual-risk note

**Status:** Compensating control for a widened-autonomy Claude Code posture (scanner
+ optional CI gate). Adapt the specifics below to your own project; this file
documents the MECHANISM, not a specific accepted-risk list.

If your project handles sensitive data (financial, health, PII, credentials), you
may want a stricter baseline than the default below — that call is project-dependent
and should be made deliberately, then recorded here the same way this template does.

The widened-autonomy posture below is a deliberate operator decision for projects
that choose to give Claude broad file/secret/MCP access. The scanner's job is to
keep that widened surface **bounded and observable** — to demonstrate that autonomy
stayed in scope. It is **not** a re-lock: the scanner flags the accepted risks, it
does not revert them.

---

## What changes under a widened-autonomy posture (the threat-model delta)

A locked-down `.claude/settings.json` deny-list typically backstops autonomy with
file-read denies, e.g.:

```jsonc
"deny": ["WebSearch", "WebFetch",
         "Read(**/.env)", "Read(**/.env.local)",
         "Read(**/.env.*.local)", "Read(**/secrets/**)"]
```

If a project deliberately lifts the `.env` / `secrets` read-denies (keeping only
`WebSearch` / `WebFetch` denied, say) and puts a `secrets` MCP (read/write) or direct
`.env` read/write into Claude's reachable tool surface, **the permission deny-list is
no longer the load-bearing control.** This config scanner is.

`enableAllProjectMcpServers: true` in `settings.local.json` compounds this — it
auto-enables any new project MCP without an explicit allowlist step.

---

## What widened autonomy typically exposes (accepted residual risk — project-specific)

- **`.env` read/write + a secrets MCP read/write reachable.** A successful
  prompt-injection (poisoned MCP tool description, a malicious file Claude reads, a
  tampered `CLAUDE.md`) could read real secrets and write them somewhere exfiltratable,
  with no deny-list to stop the read.
- **Hooks run unguarded by file-read denies.** Every script in `.claude/hooks/` executes
  in the operator's shell on tool events; a mutated hook can read whatever the old
  denies used to hide. Hooks are the highest-value injection target once file denies
  are lifted.
- **`enableAllProjectMcpServers: true`** — a new project MCP added via a malicious PR to
  `.mcp.json` auto-enables (supply-chain entry point).
- **Unpinned `npx -y ...@latest` MCP servers** auto-pull whatever the registry serves
  (poisoned-update / typosquat vector, worse once secrets are in scope).

**Which of these you accept, and why, is a project-specific decision** — write your
own list here (mirroring the shape above) rather than inheriting a generic one. The
scanner is allowed to flag accepted items like `enableAllProjectMcpServers` or lifted
denies; it must not silently modify them and must not silently re-add denies you
deliberately removed. (An explicit `enabledMcpjsonServers` allowlist in place of
`enableAllProjectMcpServers: true` is a lower-risk alternative worth considering.)

---

## Compensating-control map (how the scanner mitigates it)

| Residual risk                                                               | Control                                            | Catches it how                                                                             | Where                                     |
| --------------------------------------------------------------------------- | --------------------------------------------------- | -------------------------------------------------------------------------------------------- | ------------------------------------------ |
| Permission `allow` widening / deny dropped / `autoApprove` creep            | AgentShield `permission-review`                    | Static diff of `settings.json` — new `Bash(*)` or removed deny is high-severity → CI fails | PR gate (`high` + fail-on-findings)       |
| Hook injection (exfil, `curl\|sh`, new silent suppression)                  | AgentShield `hook-injection` (rule set)             | Static scan of `.claude/hooks/**` on any PR that touches them                              | PR gate                                   |
| `CLAUDE.md` prompt-injection (hidden unicode, auto-run, output-suppression) | AgentShield `agent-config` / CLAUDE.md ruleset     | Flags new auto-run / hidden-instruction patterns vs baseline                               | PR gate                                   |
| MCP rug-pull / tool-poisoning (description mutated post-approval)           | Invariant `mcp-scan --local-only` tool-pinning     | Hash of each tool description vs whitelist → mutation alerts                               | Nightly box cron (needs live MCP servers) |
| MCP supply-chain (unpinned `npx -y`, postinstall scripts)                   | AgentShield `--supply-chain[-online]`              | Provenance + npm registry metadata on MCP package refs                                     | Nightly                                   |
| New project MCP auto-enabled                                                | AgentShield `mcp-audit` + `.mcp.json` path trigger | Any `.mcp.json` change runs the gate; `enableAllProjectMcpServers` flagged                 | PR gate                                   |

Two tools, two layers:

- **AgentShield (`ecc-agentshield`)** — static config scan (PR gate + nightly). Runs
  anywhere; no MCP servers required.
- **Invariant `mcp-scan --local-only`** — live tool-pinning / rug-pull detection. Runs
  only where the MCP servers are reachable (operator box), so it lives in a **nightly
  box cron**, not the PR gate. `--local-only` keeps your MCP tool metadata (incl. any
  high-privilege servers) on-box — never shipped to a third-party API.

Cisco's `cisco-ai-mcp-scanner` is a reasonable alternative to evaluate (overlaps
mcp-scan, adds a second Python toolchain, best engines need API keys). YARA-only
mode is a documented escape hatch if mcp-scan's local accuracy proves insufficient.

---

## Explicitly NOT covered (accepted)

- **Runtime exfiltration during a single autonomous session** is out of scope — the
  scanner is static + nightly, not an inline proxy (`mcp-scan proxy` rewrites
  system-wide MCP configs and is invasive). The inline controls remain your existing
  `PreToolUse` hooks (e.g. `steer-bash.sh`, any `protect-*.sh`). Revisit `mcp-scan
  proxy` or a `PreToolUse` secrets-egress hook only if runtime exfil becomes a
  concern.
- **Any high-privilege MCP you deliberately keep (e.g. a secrets or cloud-project
  MCP) is intentionally high-privilege.** Its risk is accepted and whitelisted; the
  control is change-detection (rug-pull), not capability-removal.
- **`mcp-scan --local-only` is lower-accuracy** than the API mode (the privacy trade).
  Tool-_hash_ drift is still high-signal; LLM-policy flags are advisory leads to
  inspect via `uvx mcp-scan@latest inspect`, never auto-block.

---

## Invocation

- **Local / `/security-scan`:** `bash scripts/security-scan.sh [min-severity]` (default
  `medium`, non-blocking). Runs AgentShield + the advisory regression gate vs the
  baseline + (if `uvx` present and MCP servers reachable) advisory `mcp-scan --local-only`.
- **CI gate:** a workflow path-scoped to `.claude/**`, `.mcp.json`, `CLAUDE.md`, hooks;
  archives the JSON report as a build artifact; the hard gate is the AgentShield
  regression gate (`--gate --baseline .agentshield/baseline.json`) which fails the job
  **only on a NEW finding vs the committed baseline** (exit 3), so pre-accepted findings
  do not block — only drift does. Recommended **advisory (non-required) for a few
  weeks**, then promote to a required branch-protection check once the false-positive
  rate is known.
- **Config + baseline:** `.agentshield.json` (repo root) records the intended
  `failOn: high` + excludes + rule toggles (advisory; the installed CLI is flag-driven).
  `.agentshield/baseline.json` is the committed accepted-findings baseline — re-baseline
  intentionally (a reviewed event) whenever the config surface changes:
  `npx ecc-agentshield scan --path . --save-baseline .agentshield/baseline.json`.

### Installed-CLI contract (verify against your installed version)

Confirm the shipped `ecc-agentshield`'s actual flags before wiring CI — versions
have differed on format flags (`--format` = terminal|json|markdown|html vs SARIF),
output flags, and the gate mechanism (`--gate --baseline` vs `--fail-on-findings`).
Typical exit codes: **0** none, **2** findings present (NOT a failure), **3**
regression vs baseline (the only code that should fail CI).

## False-positive budget (expected, pre-triaged)

MCP-scanner-class tooling is high-FP by design — most flags describe _intended_
behavior. Treat the first scan as a **baseline, not a gate**: classify every flag
(real / accepted-with-note / FP), encode accepted items in `.agentshield.json`
excludes/toggles + the mcp-scan whitelist with a **dated comment per suppression**.
The CI gate fires only on **new** findings vs the baseline, so day-2 noise should be
near zero. Common expected flag categories: unpinned `npx -y` MCP servers (pin the
version to clear it legitimately); a secrets-access MCP's "sensitive data access"
finding (intended, if you accept it); "remote HTTP transport" MCPs (intended); hooks
using `2>/dev/null` / `|| true` (benign noise-suppression — whitelist the known ones,
keep the rule for NEW suppressions); `CLAUDE.md` "Always …" directives (legitimate —
whitelist by line/rule). Anything like `enableAllProjectMcpServers: true` is a
**genuine** finding to record as accepted residual risk, not muted as an FP.

## Audit-trail artifacts

The per-run `agentshield.json` (archived as a CI build artifact), the committed
`.agentshield/baseline.json` (the reviewed accepted-findings set + its git history of
re-baseline events), the dated suppressions in `.agentshield.json`, the mcp-scan
whitelist of pinned tool hashes, and the nightly scanner log history on the operator
box (e.g. `.claude/logs/config-sec-*.log` / `mcp-scan-*.log`, if you wire that cron).
