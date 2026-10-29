#!/usr/bin/env bash
# Mnemosyne cron stack — codebase MCP incremental re-index (data-plane, NO claude -p).
#
# Invokes the existing incremental indexer inside the codebase-mcp container. The indexer walks the
# read-only /repos:ro mount, computes a per-file content_sha256, and re-embeds ONLY files whose hash
# changed — so a nightly run is already incremental and costs embedding calls only for changed files.
# Routing this through an LLM would add a turn budget, an auth surface, and non-determinism for zero
# benefit; this is a deterministic data job, so we call the indexer directly. Tool scoping is satisfied
# by construction: the indexer only reads /repos:ro and writes its own codebase.* tables.
#
# Configure MNEMOSYNE_COMPOSE_DIR (where the Mnemosyne stack docker-compose.yml lives) and
# PROJECT_REPO_ID (the repo id this project is indexed under in the codebase MCP) before wiring
# the cron line below.
#
# Invoked from the box user crontab with flock + timeout applied on the cron line:
#   40 3 * * * flock -n /tmp/mnemo-reindex.lock -c '/path/to/your/project/.claude/scripts/cron/mnemo-reindex.sh' >> $HOME/.mnemo-reindex.log 2>&1
#
# timeout 1800 (30 min) caps a worst-case cold reindex; an incremental nightly is seconds-to-minutes.
# -T disables the TTY (cron has none). If the Mnemosyne stack is down, `docker compose exec` fails
# fast and non-zero (logged) rather than hanging — a missed nightly self-heals on the next tick.
set -euo pipefail

MNEMOSYNE_COMPOSE_DIR="${MNEMOSYNE_COMPOSE_DIR:?set MNEMOSYNE_COMPOSE_DIR to the Mnemosyne stack compose directory}"
PROJECT_REPO_ID="${PROJECT_REPO_ID:?set PROJECT_REPO_ID to this project repo id in the codebase MCP}"

cd "$MNEMOSYNE_COMPOSE_DIR"
timeout 1800 docker compose exec -T codebase-mcp node dist/indexer.js "$PROJECT_REPO_ID" "$PROJECT_REPO_ID"
