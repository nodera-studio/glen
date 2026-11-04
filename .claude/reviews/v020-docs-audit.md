# glen v0.2.0 documentation audit (main 582f77b, 2026-11-03)

Audited by 3 agents (2 Claude + 1 Codex scanner) plus synthesis. Findings only; nothing edited.
Totals: 0 CRITICAL, 0 HIGH, 4 MEDIUM, ~20 LOW. No v0.2 headline claim is wrong.

Machine-checked and passing (run by the product auditor): json_contract, mcp_contract, skill_contract, readme tests.
Verified PASS: compat.json min=max_tested=tested=18.14.0 and the omp-rpc tag pin; doctor --json (14 checks, supported range, untested/unsupported codes);
wait --for reply, inbox --wait flags and exit 10/11/130 as ok:true; task_state mapping; --warm cap 10m, release flags, state_detail;
MCP: exactly 14 tools, protocol versions 2025-11-25/2025-06-18, limits per A22, --channel hidden; __acp hidden everywhere;
omp-latest.yml cron and jq selector match A19; AGENTS Commands block matches ci.yml and lefthook.yml.

## MEDIUM

M1 wrong | docs/design.md:642 (A19), src/omp/compat.rs:2 and :93
- Claim: compat.json "is the single source, which Rust, the nightly and the site read."
- Evidence: the site does not read it. site/src/data/site.ts:4 hardcodes testedOmp '18.14.0'; site/tests/site.spec.ts:551 and site/scripts/check-links.test.mjs also hardcode it. Only Rust and omp-latest.yml:113 read compat.json.
- Fix: import compat.json in site.ts with a drift check, or reword to "site is hand-synced".

M2 missing | site/src (Hero.astro, WhatItDoes.astro:43, Agents.astro:6-12, FAQ.astro:8-12), site/src/data/site.ts:6
- Site has no v0.2 content: no wait --for reply, inbox --wait, glen mcp, --warm / release, supported range. version still '0.1.1'. FAQ 01 and the exit-10 row ("Still running. Run glen wait <job> later") omit the `woke` case.
- Fix: in the W7 site pass add those lines, change the exit-10 row to cover data.woke / data.next, bump version with the release.

M3 missing | .claude/CLAUDE.md:23 (profile dependency cell)
- Cargo.toml:22 unicode-segmentation and :29 tempfile (dev) not listed; rustix (Cargo.toml:33: fs, net, process, pty, termios) and xxhash-rust (:25: xxh3, xxh64) under-described; second example fake_ext (Cargo.toml:41-42, tests/support/fake_ext.rs) not mentioned there or in AGENTS.md.

M4 stale | .claude/agents/README.md:216-239
- Slash command table omits /explore, /goal, /auto-mode, /security-scan, /codex-advisor (all in .claude/commands/). Fix: add rows or point to commands/README.md.

## LOW

- L1 misleading | README.md:89: envelope and "every exit code" link to design.md#a10-output-contract; exit codes are in design.md A11 (:418) and docs/json.md:27.
- L2 stale | docs/design.md:855 (A23 carry-over): says src/mcp/tools.json "does not exist yet"; it exists (W5 shipped).
- L3 stale | docs/design.md:437-438 (A11 rows 129/130/143): omit that `glen mcp` (and doctor --probe) exit with those signal codes (design.md:745, json.md:1212).
- L4 stale | docs/json.md:39, design.md:305,432, skill/glen/SKILL.md:37: exit 8 not_implemented documented, but ErrorCode::NotImplemented (src/error.rs:16,56) is never constructed. Mark reserved (check skill_contract before editing).
- L5 misleading | docs/design.md:650: nightly "on pull requests that touch only itself"; omp-latest.yml:13-14 triggers on any PR touching the file.
- L6 misleading | CHANGELOG.md:33: "The CLI is unchanged." contradicts the same Unreleased section (release, mcp, --warm, --for, inbox --wait, doctor --probe).
- L7 missing | AGENTS.md:73-75: lefthook runs tests via .claude/scripts/suite-lock.sh; gitleaks optional; ci.yml also has linux-sandbox, msrv, site-deploy jobs plus flake-proof.yml/omp-latest.yml/release.yml.
- L8 stale | docs/design.md:256: cites src/rpc/worker.rs:1396-1404; abort mapping is now at 1548-1556 (Codex).
- L9 misleading | SKILL.md:38: exit 10 row points only at `glen wait <job>`; inbox --wait also exits 10 without a job (covered at SKILL.md:84-86).
- L10 misleading | .claude/CLAUDE.md:22: "stable toolchain"; rust-toolchain.toml:4 pins 1.99.1, MSRV 1.89.
- L11 stale | .claude/CLAUDE.md:82: /run-ci "same checks as ci.yml"; local pipeline lacks linux-sandbox, msrv, site-deploy, shellcheck, budgets/links/lhci. Other workflows unmentioned in .claude/CLAUDE.md.
- L12 missing | .claude/CLAUDE.md:31: site test row lists only test:e2e/test:unit; CI runs verify, budgets, links, lhci.
- L13 stale | .claude/agents/README.md:33,221: research agent "Playwright"; agents/research/agent.md:14 says never Playwright.
- L14 misleading | .claude/agents/README.md:12 vs :139-148, :269 and .claude/CLAUDE.md:131: "Claude only for planning..." but planning/error-audit are Claude || Codex.
- L15 stale | .claude/agents/README.md:3,289,322-323: generic "drop into any project / fill in" wording after bootstrap ran (unverified whether blocks are filled).
- L16 stale | .claude/skills/omp/SKILL.md:127,146: "in v0.1.0"; repo is 0.1.1 heading to 0.2.0 (behaviour still current, scripts/omp-engine.sh:180).
- L17 stale | .claude/skills/release/SKILL.md:92: step to set releaseAvailable: true; site/src/data/site.ts:6 already true. Should say bump `version`.
- L18 missing | AGENTS.md product direction ("No MCP server needed") and .claude/CLAUDE.md: no mention of optional `glen mcp`.
- L19 stale (Codex) | .claude/agents/README.md:232 references prompts/prompt-engineering.md; now .claude/prompts/prompt-engineering-claude.md and -openai.md.
- L20 stale (Codex) | .claude/scripts/README.md:18 references .claude/themes/linear-dark.json, which does not exist.
- Cosmetic: README.md:13 and design.md:3 read "18.14.0 to 18.14.0" (accurate, awkward); agents/README.md tree omits per-workflow READMEs; hooks/README.md lists Stop though none wired (line 28 says so).

## Scores
docs/json.md 9.5; skill/glen/SKILL.md 9.5; README.md 9; CHANGELOG.md 9; AGENTS.md 9; docs/design.md 8.5; site/ copy 7.5;
.claude/CLAUDE.md 8; agents/README.md 6; commands, hooks, skills READMEs 9; skills/release 8; skills/omp 8; branching-model 10.
Overall about 8.8.

## Recommendations
1. Fix M1 (site compat claim) and add v0.2 copy to site (M2) before tagging.
2. Sweep design.md stale spots (L2, L3, L5, L8), README anchor (L1), CHANGELOG sentence (L6).
3. Refresh .claude/ index drift (M3, M4, L13, L19, L20).
