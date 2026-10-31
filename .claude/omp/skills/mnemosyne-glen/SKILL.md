---
name: mnemosyne-glen
description: Recall Glen decisions and supporting memory through Mnemosyne's read-only tools.
---

<!-- Managed by omp-profile-setup.sh: mnemosyne-glen -->

# Mnemosyne memory for Glen

Mnemosyne is the owner's decision store. Always pass `project: "glen"` on every call.
Before reviewing or debugging, recall relevant decisions and follow their constraints.
Use these six read tools (exposed with the `mcp__mnemosyne_` prefix):

- `memory_search {project: "glen", query, limit}` recalls by topic or meaning.
- `memory_get {project: "glen", id, full: true}` reads the complete known memory.
- `memory_list {project: "glen", sourceKind: "decision", limit}` enumerates decisions;
  use `sourceKind: "tech-debt"` for known debt.
- `memory_get_recent {project: "glen", limit}` reads recently created memories.
- `memory_get_entity {project: "glen", nameOrId}` reads an entity and its links.
- `memory_decision_chain {project: "glen", id}` follows a decision's supersession history.

Search first, then retrieve relevant ids with `full: true` before relying on a snippet.
Cite memory ids in findings and distinguish remembered decisions from current code evidence.
The write tools `memory_store`, `memory_update`, and `memory_delete` are blocked in this
profile. Report proposed memory changes to the calling agent. Never include secret values.
