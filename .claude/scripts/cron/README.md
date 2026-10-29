# Mnemosyne cron stack — install reference

Box-side nightly automation for the Mnemosyne stack (codebase MCP re-indexing, and
optionally the learned-rules distillation loop). Reference artifacts only —
**nothing here is installed automatically.** Review, then append the lines you want
to your own crontab.

## Files

| File                  | Role                                                                                                                                                              |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `mnemosyne.crontab`   | Reference copy of example cron lines + a slot map. **Do NOT `crontab <file>` this** — it would clobber anything else already in your crontab. Append only the lines you want. |
| `mnemo-reindex.sh`    | Codebase MCP incremental re-index job body — `docker compose exec` into `codebase-mcp`, runs the incremental indexer. Reads `MNEMOSYNE_COMPOSE_DIR` and `PROJECT_REPO_ID` env vars (fails loudly if unset). flock/timeout applied by the cron line. |

The learned-rules distillation loop (`scripts/mnemosyne/distill.sh`,
`weekly-digest.sh`, `promote.mjs`, `retire.mjs`) is documented separately in
`docs/LearnedRulesLoop.md` — those scripts are cron-invokable too, but they read/write
your project's own Postgres (`DATABASE_URL`), not the Mnemosyne server's database.

## Install

1. Confirm the reindex script is executable:

   ```bash
   chmod +x /path/to/your/project/.claude/scripts/cron/mnemo-reindex.sh
   ```

2. Pre-create the log file so a first-run `tail` doesn't error (cron `>>` would create it anyway):

   ```bash
   touch "$HOME/.mnemo-reindex.log"
   ```

3. Open your crontab and **append only the lines you want** from `mnemosyne.crontab`,
   filling in `MNEMOSYNE_COMPOSE_DIR`, `PROJECT_REPO_ID`, and the project path
   placeholders. Leave any existing cron lines (timezone, other jobs) untouched.

   ```bash
   crontab -e
   ```

4. Verify:

   ```bash
   crontab -l
   ```

## Dry-run before trusting the schedule

Run the reindex job body by hand once, with the env vars set:

```bash
MNEMOSYNE_COMPOSE_DIR=/opt/mnemosyne PROJECT_REPO_ID=your-repo-id \
  /path/to/your/project/.claude/scripts/cron/mnemo-reindex.sh
```

Expect a `done <repo-id>: N indexed, M skipped` line; check the `code_index_status`
MCP tool that `last_indexed` advanced.

## If you're running fully local (Mnemosyne + Claude Code on the same machine)

You do NOT need any poll-based reindex workaround. The poll-based pattern (fetch a
remote branch on an interval, reindex only if it advanced) exists solely to bridge a
tunnel-only, no-CI-runner network boundary between a Mnemosyne box and a separate
CI/dev environment. If Mnemosyne and Claude Code run on the same machine, there's no
network-isolation problem to solve — trigger `mnemo-reindex.sh` directly from a local
post-merge git hook, or a simple cron line that does `git fetch && reindex-if-changed`
against your own working tree.

## What's intentionally NOT here

- **A staging/remote-branch poll-reindex script.** That pattern only earns its
  complexity when the Mnemosyne box has no direct network path to your CI/git host
  (tunnel-only) and needs to reconcile deletions on a schedule. See the section above
  for the local-only equivalent.
- **A memory-consolidation dedup digest.** Read-only weekly dedup tooling against the
  Mnemosyne MCP server's OWN internal Postgres is part of the Mnemosyne server project
  itself, not this harness — look for it in your Mnemosyne server repo's own tooling,
  not here.
