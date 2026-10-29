# Claude Code Setup — Harness Configuration

How the Claude Code harness in this repo is configured, why specific env vars are
set or **rejected**, and the per-machine bring-up (Linux + Mac). Verified against
Claude Code **v2.1.185**, model **Opus 4.8 (1M)**, **Claude Max** subscription —
re-verify against current docs if you're on a different version/model/plan.

## Config layer split

| Layer                                | Path                                                                        | Holds                                                                             | Synced via                            |
| ------------------------------------ | --------------------------------------------------------------------------- | --------------------------------------------------------------------------------- | ------------------------------------- |
| Repo (shared)                        | `.claude/settings.json`                                                     | project hooks, permissions, statusline, `env` block                               | git                                   |
| Repo (shared)                        | `.claude/CLAUDE.md`, `.claude/hooks/`, `.claude/scripts/`, `.claude/rules/` | project behavior                                                                  | git                                   |
| User (per-machine)                   | `~/.claude/settings.json`                                                   | identity: `model`, `effortLevel`, `outputStyle`, theme, plugins, OAuth-bound MCPs | manual / dotfiles                     |
| Local (per-machine, **git-ignored**) | `.claude/settings.local.json`                                               | machine-local permission allows + `enabledMcpjsonServers`                         | **not** in git (add to `.gitignore`)  |
| Per-machine tools                    | CLIs (rg, ast-grep, …)                                                      | the productivity CLI layer                                                        | `bootstrap-tools.sh`                  |

Cross-machine delivery = commit repo `.claude/` + run `bootstrap-tools.sh` on each
box. **No plugin packaging** for the always-on hooks/rules layer — re-pathing
working hooks into a plugin would risk a silent-disable failure mode, for no
benefit over git. (The `plugin/` packaging in this repo is an alternate delivery
path for the commands/agents/skills layer, not a replacement for the git-synced
hooks/rules — see the repo README.)

**OAuth-bound MCPs** (claude.ai-managed connectors — a database platform, an error
tracker, an issue tracker, a payments platform, and similar) live at the user/account
layer, not in this repo at all — there's no file to commit for them. See
`skills/README.md`'s "Claude connectors are account-specific" section for how to
document the routing/usage pattern for one without pretending it's portable.

## Env var decision matrix

Applied in `.claude/settings.json` `"env"` (keep the file `jq empty`-valid):

```jsonc
"env": {
  "ENABLE_LSP_TOOL": "1",
  "DISABLE_TELEMETRY": "1",            // individual flag — NOT the bundled killer (below)
  "BASH_DEFAULT_TIMEOUT_MS": "300000", // 5 min default (was 2 min); confirmed working in 2.1.185
  "BASH_MAX_TIMEOUT_MS": "600000"      // 10 min ceiling
}
```

**`ENABLE_LSP_TOOL`** turns on Claude Code's built-in `LSP` tool — `goToDefinition`,
`findReferences`, `hover`, `documentSymbol`, `workspaceSymbol`, `goToImplementation`,
`prepareCallHierarchy`, `incomingCalls`, `outgoingCalls`. This is precise, ~50ms code
navigation against a real language server, not a grep-based approximation — reach for
it once a symbol is located (`rg`/`ast-grep` find the spot; `LSP` then answers "what
calls this" or "where's this defined" exactly, no false positives from a same-named
symbol elsewhere). It's a no-op without a language server actually running: install the
official plugin matching your language(s) from `claude-plugins-official` —
`typescript-lsp` (TS/JS), `pyright-lsp` (Python), `gopls-lsp` (Go), `rust-analyzer-lsp`
(Rust), `clangd-lsp` (C/C++), `jdtls-lsp` (Java), and others exist for most mainstream
languages (`/plugin install <name>@claude-plugins-official`). These also turn on
automatic diagnostics — after every edit, the language server reports type errors and
missing imports back to Claude in the same turn, without running a separate
typecheck. No language server installed = the flag does nothing and `LSP` tool calls
fail; that's expected, not a bug. `/bootstrap-project` doesn't install this for you —
add it yourself once you know the project's primary language(s).

### Other official plugins worth knowing about

Not part of this harness (they're user/account-level installs, same as the LSP
plugins above), but genuinely complementary — install them per-project as needed:

- **`claude-code-setup`** — Anthropic's own codebase-analyzer: scans a project and
  recommends MCP servers, skills, hooks, subagents, and slash commands tailored to
  its actual stack. Read-only, never modifies files. Run it alongside
  `/bootstrap-project` for a second, broader opinion — this harness's bootstrap fills
  in `{{TOKEN}}`s for what it already ships; `claude-code-setup` surfaces automation
  ideas this template doesn't cover at all (e.g. "you're on FastAPI, consider a
  Python-REPL MCP"). Ask it to "recommend automations for this project."
- **`skill-creator`** — walks you through writing and validating a new skill (test
  cases, an iterate-until-it-works loop) — useful when authoring the project-specific
  skills `skills/README.md` asks you to add.
- **`frontend-design`** — production-grade UI guidance that avoids generic/AI-default
  aesthetics — worth installing alongside (or instead of) a hand-authored
  design-system skill if your project has meaningful frontend surface.

**Rejected — deliberately NOT set (each verified against current docs):**

| Var                                        | Why rejected                                                                                                                                                             |
| ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `MAX_THINKING_TOKENS`                      | No-op on Opus 4.8 — it uses adaptive reasoning, which ignores numeric budgets. Only `effort` moves reasoning depth on this model. (`=0` would just kill thinking.)       |
| `ENABLE_PROMPT_CACHING_1H`                 | No-op on a Max subscription — the 1h cache TTL is already requested automatically. Only matters on API-key / Bedrock / Vertex billing.                                   |
| `CLAUDE_CODE_SUBAGENT_MODEL`               | Would **override every agent's `model:` frontmatter**, breaking the tuned agent suite (orchestrators forbid overriding agent models). Use per-agent frontmatter instead. |
| `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` | Bundles `DISABLE_AUTOUPDATER` → silently stops security updates. Use the individual `DISABLE_TELEMETRY` instead.                                                         |
| `CLAUDE_CODE_SHELL`                        | Moot when bash is already the default shell. (Worth setting on a Mac whose login shell is zsh, pointing at a plain bash, only if snapshot breakage appears.)             |
| `DISABLE_NON_ESSENTIAL_MODEL_CALLS`        | Not a confirmed env-var name in current docs — do not use.                                                                                                               |

## Subagent model routing — invariant

**Per-agent frontmatter is authoritative. Never set a global subagent model.** The
agent suite intentionally mixes `model: inherit` (frontier roles) with pinned
cheaper models (mechanical roles) and per-role `effort`. A global
`CLAUDE_CODE_SUBAGENT_MODEL` would flatten all of that. This survives "optimization"
passes by being written down here.

## CLI tooling — `bootstrap-tools.sh`

`.claude/scripts/bootstrap-tools.sh` installs the productivity CLI layer.
Idempotent, OS-detecting, best-effort (records a summary, never aborts).

```bash
.claude/scripts/bootstrap-tools.sh --dry-run   # show the plan
.claude/scripts/bootstrap-tools.sh             # install what's missing
```

- **Linux** (no cargo/rustup/brew): apt for shellcheck, git-delta, universal-ctags,
  miller, hyperfine; prebuilt GitHub-release binaries for the rest (→
  `~/.local/bin` or `/usr/local/bin`).
- **macOS**: Homebrew.
- **`yq`**: fetched as the **mikefarah v4** binary — NOT the apt `yq` (a python jq
  wrapper, v3). Don't `apt install yq`.
- **`ast-grep`**: ships an `sg` alias that **collides with util-linux `/usr/bin/sg`**.
  Always invoke `ast-grep` by full name. The script never creates an `sg` alias; to
  shorten it, add `alias astg='ast-grep'` to your shell rc yourself.
- Wires `delta` as git's pager (per-machine, idempotent — leaves an existing
  non-delta `core.pager` untouched).
- Typically already present: rg, fd/fdfind, jq, fzf, gh, semgrep, bunx, npx.

If installing to `~/.local/bin`, ensure it's on `PATH` (add to `~/.bashrc` non-
interactive section or `~/.zshenv` so the Bash-tool snapshot inherits it).

### Why these tools — the token/context-savings rationale

Every tool this script installs replaces a default CLI habit with one that returns
less noise, more structure, or a deterministic result — which matters because every
byte a tool returns is context the agent has to read. This is the routing table:
`hooks/steer-bash.sh` actively enforces the top few (blocks the old habit, names the
replacement); the rest are conventions worth following even though nothing blocks
the old way.

| Instead of... | Use | Why |
| --- | --- | --- |
| `grep` for in-repo search | `rg` (ripgrep) | Faster, respects `.gitignore` by default — no manually excluding `node_modules`/`.git` from every search. |
| `find -name`/`-path` | `rg --files \| rg <pattern>` | One process instead of a `find` + pipeline; same `.gitignore`-aware behavior. Enforced by `steer-bash.sh` (operational `find` — `-exec`/`-delete`/`-mtime` — stays allowed). |
| `sed -i` (in-place edit) | The Edit tool, or `sd` for one-off CLI find/replace | `sed -i`'s backreference syntax and platform quirks (BSD vs GNU `-i`) are a common source of silent no-ops; the Edit tool is deterministic and reviewable, `sd` has a simpler literal/regex syntax for quick CLI use. `sed -i` is blocked outright by `steer-bash.sh`. |
| `cat`/`head`/`tail` of a code file | The Read tool | Structured, cacheable, supports `offset`/`limit` for large files without dumping the whole thing into context. Blocked for code files (not heredocs, not logs) by `steer-bash.sh`. |
| Multi-line regex `grep`/`sed` for a code SHAPE (not a literal string) | `ast-grep` | Language-aware structural match/rewrite — matches by AST shape, not brittle regex escaping. Invoke as `ast-grep` (its `sg` alias collides with util-linux's `sg`). |
| `diff`/plain `git diff` to READ a change | `difft` (`difft a b`, or `GIT_EXTERNAL_DIFF=difft git diff`) | Structural diff — ignores pure reformatting/whitespace reflow, shows the actual semantic change. Plain `git diff` stays the tool for generating a patch to apply. |
| The default git diff pager | `delta` | Syntax-highlighted, side-by-side, easier to read in a terminal. Wired automatically by the installer if you haven't already set a pager. |
| Manual `sed`/awk edits to YAML/JSON | `yq` (mikefarah v4 — NOT the apt python `yq`) | Structural, format-preserving edits — doesn't reflow the file the way a naive text substitution can. |
| Reading/grepping deeply nested JSON by eye | `gron` | Flattens JSON to greppable `path = value` lines: `gron f.json \| rg X \| gron -u` round-trips back to JSON. |
| Reading many files just to gauge repo size/complexity | `scc` | One table (LOC, file count, estimated complexity) instead of opening files one at a time. |
| Grep-based go-to-definition / find-references / rename | The `LSP` tool (`ENABLE_LSP_TOOL=1`) | Exact, ~50ms, backed by a real language server — no false positives from a same-named symbol elsewhere. Needs a language server installed (e.g. the `typescript-lsp` plugin) — reach for it once `rg`/`ast-grep` has located the symbol. |
| `curl`/`wget` to an external URL | The Exa MCP (web content) or a service's own SDK/CLI (a known API) | `WebFetch` is off by design in this harness's `settings.json`; ad-hoc scraping bypasses the tools built for this. Loopback stays allowed. Enforced by `steer-bash.sh`. |

The remaining tools in the install list (`git-absorb`, `hyperfine`, `watchexec`,
`dust`, `procs`, `hexyl`, `lazygit`, `mlr`, `ctags`, `gitleaks`, `shellcheck`) are
general productivity/security tooling rather than direct token-saving replacements —
useful to have, not routed-to by any hook.

## Manual steps NOT carried automatically

### 1. `.claude/settings.local.json` is git-ignored — create it by hand (per machine)

```jsonc
{
  "permissions": { ... },
  "enabledMcpjsonServers": ["your-mcp-server"],
  "enableAllProjectMcpServers": true
}
```

Verify: `jq empty < .claude/settings.local.json && echo OK`.

### 2. Install the CLI layer (per machine)

Run `.claude/scripts/bootstrap-tools.sh` on each machine (`--dry-run` first to
preview). The `settings.json` `env` block + any `PostToolUse` hook wiring travel
with the repo — no manual settings.json edit needed beyond what's already committed.

## Mac bring-up sequence

1. `git pull` (gets repo `.claude/` incl. hooks, scripts, settings, this doc).
2. `.claude/scripts/bootstrap-tools.sh` (brew path).
3. Recreate `.claude/settings.local.json` (git-ignored) with your local allows.
4. Confirm `~/.claude/settings.json` identity (`model`, `effortLevel`, `outputStyle`).
