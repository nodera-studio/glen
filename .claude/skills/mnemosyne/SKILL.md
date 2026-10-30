---
name: mnemosyne
description: >-
  glen's guide to Mnemosyne, the owner's memory, code index, secrets store and project board,
  reached through the `mnemosyne` plugin (tools `mcp__plugin_mnemosyne_<server>__<tool>`, servers
  memory, codebase, secrets, workspace). Use it whenever work in glen needs to recall or record a
  decision ("did we decide anything about X", "make a note so we don't forget", "store this before
  merging"), read or update a board issue (create the wave issues, move an issue to in_progress /
  in_review / done, tag it shipped:main, comment on it), look up prior omp research, check the code
  index, or read a secret. Always project "glen" for memory/code/secrets and the Glen projectId for
  workspace calls. Routes away from the wrong tool: literal matches in this repo are `rg`; issues
  are never Linear or GitHub Issues; throwaway notes are not memories.
---

# Mnemosyne in glen

The full manual is the plugin's own skills: `mnemosyne:mnemosyne` (memory, code, secrets, the
board) and `mnemosyne:mnemosyne-workspace` (documents, comments, tags, revisions). This file
holds the glen-specific values and the calls the harness relies on. Load tool schemas with
ToolSearch before the first call (e.g. `+plugin_mnemosyne_workspace board document`).

## glen's identifiers

| What | Value |
| --- | --- |
| Memory / code / secrets `project` | `glen` (case-insensitive; the workspace shows it as "Glen") |
| Workspace `projectId` | `ff92abe1-da63-402d-a4f1-c8690e471be1` |
| Related projects | omp-as-a-donor research → `metis`; harness questions → `agent-os`; machine and toolchain setup → `ops` |

Never write to the `default` project. A hook refuses memory, code and secrets calls without
`project`; retry with the project it names.

## Memory — recall before, record after

- **Before substantial work:** `memory_search {project: "glen", query}` on the topic, and
  `memory_list {project: "glen", sourceKind: "decision"}` (also `"tech-debt"` for the area).
  Follow the decisions you find; if the work contradicts one, say so before proceeding.
- **When done:** `memory_store {project: "glen", type, title, content, sourceKind?, tags?,
  importance?, metadata?, decidedIn?, relatedIds?, supersedesId?}`. Search first and
  `memory_update` an existing memory instead of duplicating.
  - `type`: `semantic` (fact or decision), `procedural` (how-to), `episodic` (what happened),
    `entity`.
  - `sourceKind`: `decision`, `plan`, `implementation`, `tech-debt`, `research`, `audit`,
    `requirements`.
  - Tags lowercase kebab-case; reserved tags `architecture`, `security`, `performance`, `ops`,
    `product` are spelled exactly. An architectural decision is `sourceKind: "decision"` and
    `tags: ["architecture", ...]`.
  - File paths go in `metadata.file`; tech debt uses `metadata: {debtId, severity, category, file}`.
- **Before every `gh pr merge`:** store a memory referencing the PR (`PR #<n>` in the title,
  content or tags). The `capture-before-merge` hook blocks the merge until one exists in the
  session.
- Never store transient state, transcripts or secret values.

## Code index

`code_search {project: "glen", repo: "glen", query}` → `code_symbol_lookup` →
`code_graph_expand` → `code_get_file`. Use `code_search` to orient before planning. The index
is a snapshot (`code_index_status` shows when); inside this repo the working tree is the
truth, and a known string is `rg`.

- **How glen is indexed.** No UI or MCP tool registers a repository. glen is a row in the
  local corpus DB pointing at a clean clone of `main` at `~/Developer/.mnemosyne-index/glen`
  (memory `79a30160`, project `glen`). Never commit or check out branches in that clone.
- **After every merged PR:** `git -C ~/Developer/.mnemosyne-index/glen pull --ff-only`, then
  `code_reindex {project: "glen", repo: "glen"}`, and confirm with `code_index_status`.
  `code_reindex` is a write: run it at that point, when asked, or for `/implement`'s re-index
  step.

## Secrets

`secret_list {project: "glen"}` returns names only. `secret_get` only when a task needs the
value; never echo it into chat, logs, commits, memories or documents. `secret_set` and
`secret_delete` are denied in `settings.json`.

## The board (glen's issue tracker)

Tools are `mcp__plugin_mnemosyne_workspace__*`; every call takes the Glen `projectId` above
or a document id.

- **Shape:** modules hold issues; issues nest up to three levels; modules do not nest. Ideas
  are not the backlog (`board_promote` graduates an idea into an issue or module and links it).
- **Statuses:** `backlog` (default on create), `todo`, `in_progress`, `in_review`, `done`,
  `cancelled`.
- **Read:** `board_list {projectId, moduleId?, parentId?, kind?, statuses?, query?}` (board
  order, with `blockedBy`); `document_get {id}` (one item with its `revision`, tags, comments).
- **Create:** `document_create {projectId, kind: "modules" | "issues", title, markdown}`, then
  nest with `board_place {id, parentId}`.
- **Status change:** `document_update {id, expectedRevision, status, ...}`. Omitted fields take
  schema defaults (`markdown: ""`, `tags: []`, ...), so send the item's current `title`,
  `markdown`, `tags`, `priority`, `assigneeId` and `checklist` from `document_get` with the new
  status, then re-read once to confirm nothing was blanked.
- **Tags:** `document_tags_update {id, expectedRevision, mode: "add" | "remove", tags}` (atomic).
  glen uses `shipped:main` (merged on `main`) and `released:v0.x.y` (in that tag).
- **Comments:** `document_comment {id, expectedRevision, markdown}`.
- **Revisions:** every write carries the latest `revision`; on `CONFLICT`, re-read and retry.
- **References:** issues have no keys (the API returns only a document id). Cite `W<n>` plus the document id in PRs, commits and memories.
- **Errors:** `FORBIDDEN` / `NOT_FOUND` is the token's boundary — report it, don't work around
  it. HTTP 401 means the token is missing or expired: tell the user.

## Queues

`queue_item_add` / `queue_item_update` accept a `documentId` only for Ideas, Notes and
Research. **Never pass a board issue id as `documentId`.** The server answers
`INVALID_REQUEST` but still persists a broken row (documentKind `issues`) that makes
`queue_get` and the web queue page fail; the only recovery found was deleting and recreating
the queue. Put the issue's wave label and id in the item title instead (`W3 — <summary>
(<issue id>)`). This is a Mnemosyne server bug; the rule is the workaround until it is fixed.

## Ideas, Research, Notes

Ideas hold owner intent, Research holds cited findings, Notes hold working summaries. Use
Notes for per-wave plan summaries, wave-end handoffs and run checkpoints (`document_create
{projectId, kind: "notes", title, markdown}`), not memories and not a local file.
