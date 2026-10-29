#!/usr/bin/env node
// Mnemosyne — rule_findings writer.
//
// Requires a `rule_findings`/`rule_candidates` table pair in your project's own
// Postgres — see docs/LearnedRulesLoop.md for the schema.
//
// Inserts one (or many) finding rows into your project's own Postgres
// `rule_findings` append-only ledger. Standalone Node ESM — run directly, no app
// bootstrap. Connects via `postgres-js` using the `DATABASE_URL` env var (no
// hardcoded fallback — fails loudly if unset, since there's no project-specific
// default connection to assume).
//
// Idempotency: a deterministic dedupe on (session_id, finding_text, file) so
// re-runs of the same review/turn don't double-count evidence. Implemented as
// a guard SELECT before INSERT (these tables carry no unique constraint by
// design — the dedupe key has nullable parts).
//
// Usage:
//   DATABASE_URL=postgres://... node record-finding.mjs --json '<finding-json-object>'
//   DATABASE_URL=postgres://... node record-finding.mjs --batch <file.jsonl>      # one finding per line
//   cat findings.jsonl | DATABASE_URL=postgres://... node record-finding.mjs --batch -
//
// Finding shape (all optional except source + finding_text):
//   { source, engine, finding_text, category, file, accepted, action,
//     commit_sha, session_id }
//
// Exit 0 on success (including "nothing to insert"); exit 1 only on a hard
// connection/SQL error, or a missing DATABASE_URL. Callers (hooks) should treat
// capture as best-effort.

import { readFileSync } from 'node:fs';
import postgres from 'postgres';

const DB_URL = process.env.DATABASE_URL;
if (!DB_URL) {
  console.error(
    'record-finding: DATABASE_URL env var is required (see docs/LearnedRulesLoop.md) — no default connection is assumed',
  );
  process.exit(1);
}

const argv = process.argv.slice(2);
const flag = (name) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : undefined;
};

/** Read findings from --json or --batch into an array of plain objects. */
function loadFindings() {
  const jsonArg = flag('--json');
  if (jsonArg !== undefined) {
    const parsed = JSON.parse(jsonArg);
    return Array.isArray(parsed) ? parsed : [parsed];
  }
  const batchArg = flag('--batch');
  if (batchArg !== undefined) {
    const raw = batchArg === '-' ? readFileSync(0, 'utf8') : readFileSync(batchArg, 'utf8');
    return raw
      .split('\n')
      .map((l) => l.trim())
      .filter(Boolean)
      .map((l) => JSON.parse(l));
  }
  throw new Error('record-finding: pass --json <obj> or --batch <file.jsonl|->');
}

/** Coerce a finding to the column shape, defaulting unknowns to null. */
function normalize(f) {
  if (!f || typeof f !== 'object') throw new Error('record-finding: finding is not an object');
  if (!f.source) throw new Error('record-finding: finding.source is required');
  if (!f.finding_text) throw new Error('record-finding: finding.finding_text is required');
  return {
    source: String(f.source),
    engine: f.engine != null ? String(f.engine) : null,
    finding_text: String(f.finding_text),
    category: f.category != null ? String(f.category) : null,
    file: f.file != null ? String(f.file) : null,
    accepted: typeof f.accepted === 'boolean' ? f.accepted : null,
    action: f.action != null ? String(f.action) : null,
    commit_sha: f.commit_sha != null ? String(f.commit_sha) : null,
    session_id: f.session_id != null ? String(f.session_id) : null,
  };
}

async function main() {
  const findings = loadFindings().map(normalize);
  if (findings.length === 0) {
    console.log('record-finding: nothing to insert');
    return;
  }

  const sql = postgres(DB_URL, { max: 1, prepare: false });
  let inserted = 0;
  let skipped = 0;
  try {
    for (const f of findings) {
      // Deterministic dedupe: skip if an identical (session_id, finding_text,
      // file) row already exists. `IS NOT DISTINCT FROM` handles NULL session_id
      // / file correctly (NULL = NULL for the dedupe key).
      const existing = await sql`
        SELECT 1 FROM rule_findings
        WHERE session_id IS NOT DISTINCT FROM ${f.session_id}
          AND finding_text = ${f.finding_text}
          AND file IS NOT DISTINCT FROM ${f.file}
        LIMIT 1
      `;
      if (existing.length > 0) {
        skipped += 1;
        continue;
      }
      await sql`
        INSERT INTO rule_findings
          (source, engine, finding_text, category, file, accepted, action, commit_sha, session_id)
        VALUES
          (${f.source}, ${f.engine}, ${f.finding_text}, ${f.category}, ${f.file},
           ${f.accepted}, ${f.action}, ${f.commit_sha}, ${f.session_id})
      `;
      inserted += 1;
    }
  } finally {
    await sql.end({ timeout: 5 });
  }
  console.log(`record-finding: inserted ${inserted}, skipped ${skipped} (dedupe)`);
}

main().catch((err) => {
  console.error(`record-finding: ${err.message}`);
  process.exit(1);
});
