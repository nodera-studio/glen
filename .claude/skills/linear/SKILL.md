---
name: linear
description: >-
  Use the Linear connector (the claude.ai Linear MCP) for all issue tracking — tickets
  are `{{TICKET_PREFIX}}-###`. Reach for it whenever the user wants to look up what a ticket is, update a ticket's
  status or fields, list a team's issues, comment on an issue, or post a project status update.
  Trigger on indirect phrasing too — "what's PROJ-512 about", "move this to in-progress", "what's
  on my plate this cycle", "leave a note on the ticket", "give the team a status update", "file
  this as a bug". Linear is the single source of truth for issue tracking — never spin up a
  parallel TODO file or markdown checklist to track work that belongs in a ticket. The connector
  is the live surface (the old plugin was removed).
---

# Linear — issue tracking via the connector

Tickets use your team's ticket-ID format (e.g. `PROJ-###`). The **claude.ai Linear connector** is the live surface — the standalone
plugin was removed, so use the connector tools (`mcp__*Linear*`), not a CLI.

## Core operations

- **Read a ticket:** `get_issue` (by ticket ID, e.g. `PROJ-###`) — pull title, status, description, comments.
- **List work:** `list_issues` (filter by team / assignee / state) — "what's in this cycle".
- **Update:** `save_issue` (status, assignee, fields), `save_comment` (a note on the issue),
  `save_status_update` (a project-level status post).
- Supporting reads: `list_teams`, `list_projects`, `list_cycles`, `get_project`, `list_comments`.

## WHEN to reach for it

- Any reference to a ticket, sprint/cycle planning, or status reporting → Linear.
- **Never create a parallel TODO file** (markdown checklist, scratch list) for work that has — or
  should have — a Linear ticket. An autonomous wave loop (if configured) already creates and flips
  Linear tickets as it works; align with that loop rather than tracking state in files it can't see.

## Guardrails

- `save_*` tools mutate Linear state — they are the intended write path here (issue tracking is
  collaborative), but don't bulk-edit or change status speculatively; mirror what the user asked.
- Keep ticket bodies PII-free where the content will be quoted into commits or audit logs.
