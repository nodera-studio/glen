# Custom Theme — Linear Dark

A custom Claude Code color theme built to match [Linear](https://linear.app)'s dark UI —
indigo accents, muted greys, the same diff-added/removed tints Linear's own app uses.
Themes are a **user-level** (per-machine) setting — `~/.claude/settings.json`'s top-level
`"theme"` key — not a project-level one, so this installs to your home directory
regardless of which project you're in. See `docs/Claude-Code-Setup.md`'s config layer
split for why themes live there instead of in a project's `.claude/`.

## What it looks like

Indigo (`#5e6ad2`) for Claude's own chip/prompt-border/permission accents, teal
(`#02b8cc`) for memory/IDE indicators, the standard success/error/warning triad tuned to
Linear's palette, and diff backgrounds dark enough to read comfortably in a terminal
(`#191c33` added / `#311620` removed) rather than the harsher default green/red.

## Automatic install

```bash
scripts/apply-theme.sh
```

Copies `themes/linear-dark.json` to `~/.claude/themes/linear-dark.json` and sets
`"theme": "custom:linear-dark"` in `~/.claude/settings.json` (creating the file if it
doesn't exist yet, preserving every other key if it does). Requires `jq`. Restart Claude
Code afterward to see it.

Pass a different theme name (`scripts/apply-theme.sh <name>`) if you add more themes
under `themes/` later — it looks for `themes/<name>.json` relative to the script.

## Manual install

1. Copy `themes/linear-dark.json` to `~/.claude/themes/linear-dark.json`.
2. Add (or edit) `"theme": "custom:linear-dark"` in `~/.claude/settings.json`.
3. Restart Claude Code.

## Reverting

Set `"theme"` back to a built-in value (e.g. `"dark"` or `"light"`) in
`~/.claude/settings.json`, or delete the key entirely to fall back to Claude Code's
default. The installed `~/.claude/themes/linear-dark.json` is harmless to leave in place
either way.
