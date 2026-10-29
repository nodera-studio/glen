#!/usr/bin/env node
// Mnemosyne — manual / scripted retirement.
//
// Requires a `rule_findings`/`rule_candidates` table pair in your project's own
// Postgres — see docs/LearnedRulesLoop.md for the schema.
//
// Retires a promoted (or candidate) rule: flips status -> 'retired' and removes
// it from its tier (the claude_md line, by anchor marker). For a memory_mcp
// rule it prints the memory_delete call to paste (a node script can't call the
// memory MCP). Auto-retirement of fully-decayed rules (score <= 0) is performed
// by the nightly distiller sweep; this script is the manual / explicit path.
//
// Usage:
//   DATABASE_URL=postgres://... node retire.mjs <candidate_id> [--reason "<why>"]

import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { execSync } from 'node:child_process';
import postgres from 'postgres';

const DB_URL = process.env.DATABASE_URL;
if (!DB_URL) {
  console.error(
    'retire: DATABASE_URL env var is required (see docs/LearnedRulesLoop.md) — no default connection is assumed',
  );
  process.exit(1);
}

const here = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = (() => {
  try {
    return execSync('git rev-parse --show-toplevel', { encoding: 'utf8' }).trim();
  } catch {
    return join(here, '..', '..', '..');
  }
})();
const CLAUDE_MD = join(REPO_ROOT, '.claude', 'CLAUDE.md');

const argv = process.argv.slice(2);
const candidateId = argv.find((a) => !a.startsWith('--'));

if (!candidateId) {
  console.error('retire: usage: node retire.mjs <candidate_id> [--reason "<why>"]');
  process.exit(1);
}

function removeClaudeMdLine(anchor) {
  if (!existsSync(CLAUDE_MD)) return;
  const content = readFileSync(CLAUDE_MD, 'utf8');
  const marker = `<!-- mnemosyne:${anchor} -->`;
  const lines = content.split('\n').filter((l) => !l.includes(marker));
  writeFileSync(CLAUDE_MD, lines.join('\n'));
}

async function main() {
  const sql = postgres(DB_URL, { max: 1, prepare: false });
  try {
    const rows = await sql`
      SELECT id, status, tier, promoted_ref FROM rule_candidates
      WHERE id = ${candidateId} LIMIT 1
    `;
    if (rows.length === 0) throw new Error(`retire: candidate ${candidateId} not found`);
    const cand = rows[0];
    if (cand.status === 'retired') {
      console.log(`retire: ${cand.id} already retired`);
      return;
    }

    if (cand.tier === 'claude_md' && cand.promoted_ref) {
      removeClaudeMdLine(cand.promoted_ref);
    }

    await sql`
      UPDATE rule_candidates
      SET status = 'retired', updated_at = now() WHERE id = ${cand.id}
    `;
    console.log(`retire: ${cand.id} -> retired (was ${cand.status}, tier=${cand.tier ?? 'none'})`);
    if (cand.tier === 'memory_mcp' && cand.promoted_ref) {
      console.log(
        `retire: memory_mcp rule — call memory_delete on entity ${cand.promoted_ref} in a Claude session.`,
      );
    }
  } finally {
    await sql.end({ timeout: 5 });
  }
}

main().catch((err) => {
  console.error(err.message);
  process.exit(1);
});
