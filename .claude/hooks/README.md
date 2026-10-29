# Hooks

Enforcement scripts wired into `settings.json` against Claude Code's lifecycle events
(`SessionStart`, `PreToolUse`, `PostToolUse`, `Stop`, `PreCompact`). Two postures, used
deliberately per hook:

- **Warn-don't-block** — prints guidance, exits 0. Used where a wrong call is annoying,
  not dangerous (formatting, shellcheck nits, a reminder).
- **Hard block** — exits 2 with a message explaining what to do instead. Used only where
  the thing being blocked is a real, specific footgun (a dropped legacy code path, a
  merge with no captured decision) — never as a blanket "be careful" gate.

## What ships, wired in `settings.json`

| Hook | Event | Posture | What it does |
| --- | --- | --- | --- |
| `session-start.sh` | `SessionStart` | info | Prints a "check available skills" reminder |
| `reinject-invariants.sh` | `SessionStart`, `PreCompact` | info | Re-injects a short, project-defined invariants list so it survives context compaction — the payload is a placeholder in this template, see below |
| `block-old-codex.sh` | `PreToolUse` (`Bash`, `Skill`) | hard block | Blocks legacy/duplicate Codex dispatch paths, redirects to the canonical one |
| `steer-bash.sh` | `PreToolUse` (`Bash`) | warn (steers) | Redirects risky/inefficient bash patterns (`sed -i` → Edit tool, raw `cat`/`head` of code → Read tool, `.env` reads → secrets MCP, external `curl` → an allowlisted host check) toward better tools |
| `capture-before-merge.sh` | `PreToolUse` (`Bash`, on `gh pr merge`) | hard block | Blocks a PR merge until a durable-memory entry referencing the PR/branch exists in-session (only meaningful if you run a memory MCP) |
| `prettier-on-edit.sh` | `PostToolUse` (`Edit\|Write`) | warn-don't-block | Runs prettier on the edited file if the project has it configured |
| `shellcheck-on-edit.sh` | `PostToolUse` (`Edit\|Write`) | warn-don't-block | Lints edited shell scripts if shellcheck is installed |
| `mnemosyne-capture.sh` | `Stop` | opt-in (`MNEMOSYNE_CAPTURE=1`) | Captures an episodic fix summary into a memory MCP's findings table, if you run one |

## Fill in before this does anything project-specific

Two hooks ship as **mechanism-only placeholders** — they need your project's actual
content before they're meaningful:

- **`reinject-invariants.sh`** — the re-injected payload is a bracketed placeholder list
  ("[Add your project's non-negotiable invariants here]"). Replace it with the handful
  of rules your project genuinely cannot afford an agent to forget after a compaction —
  a data-access pattern that must never be bypassed, a testing policy, a git-hygiene
  rule. Keep it short (a few hundred tokens); this re-injects on every compaction, so
  bloat here is a recurring tax.
- **`steer-bash.sh`**'s `ALLOW_HOSTS` allowlist ships with just loopback addresses.
  Add your own trusted remote hosts (an internal API, a deploy target) if your project's
  agents legitimately need to `curl` them. This hook is the enforcement half of a
  larger set of CLI-tool replacements (`rg` over `grep`, `sd`/the Edit tool over
  `sed -i`, `ast-grep`, `difft`, `yq`, `gron`, `scc`, and more) — see
  `docs/Claude-Code-Setup.md`'s "Why these tools" table for the full routing rationale
  and `scripts/bootstrap-tools.sh` for the installer.

## Custom hooks for your project

The source project this harness was extracted from had several more hooks that
hardcoded its own dev-tool/container/monorepo specifics — dropped as dead weight, but
the pattern each demonstrated (enforcing a canonical dev-start command, protecting
generated files, rebuilding a shared package on edit, gating session-end on green
tests, installing deps in a fresh worktree) is exactly the kind of thing worth
authoring for YOUR project's actual stack. See
[`README-optional-hooks.md`](README-optional-hooks.md) for the full list with event/
posture per idea, and copy the fail-open, non-blocking-unless-warranted style of the
hooks that did ship.

## Writing your own

A hook is a script that reads a JSON payload from stdin (`tool_name`, `tool_input`,
etc. — see any shipped hook for the exact shape), and communicates back via exit code
(0 = allow/continue, 2 = block with the stderr message shown to the agent) or, for
`PreCompact`/`SessionStart`, by printing context to stdout. Wire it into `settings.json`
under the matching event array; use a `matcher` to scope it to specific tools
(`"matcher": "Bash"`, `"matcher": "Edit|Write"`) when it shouldn't fire on everything.
