# agent-os-workflow — local plugin packaging

The whole agent-os Claude Code suite (agents + skills + commands + hooks) packaged
as one versioned, toggleable plugin. **Inert by default** — the component dirs
are symlinks into the loose `.claude/{agents,skills,commands,hooks}` layout,
which remains the live configuration. Nothing loads twice.

## Load it (opt-in)

```bash
claude --plugin-dir .claude/plugin/agent-os-workflow
```

Version bumps: edit `.claude-plugin/plugin.json` → `version`.

## Two breaking behaviors when running AS a plugin

1. **Command namespace.** Every command gains the plugin prefix:
   `/implement` → `/agent-os-workflow:implement`, `/auto-mode` →
   `/agent-os-workflow:auto-mode`, etc. Muscle-memory names change — this is
   the main reason the plugin is opt-in until deliberately adopted.
2. **Subagent frontmatter restrictions.** Plugin-loaded subagents IGNORE
   `hooks`, `mcpServers`, and `permissionMode` frontmatter (security posture of
   the plugin system). None of the current agents rely on those fields, but
   don't add them expecting plugin-context behavior.

## Cutover (only if/when adopting the plugin as the primary layout)

1. Replace the symlinks with the real directories (move
   `.claude/{agents,skills,commands,hooks}` in).
2. Remove the hook wiring from `.claude/settings.json` (the plugin's own
   settings carry it) — never run both.
3. Re-teach command names with the `/agent-os-workflow:` prefix.

Until then: the loose layout is authoritative; this directory is packaging.
