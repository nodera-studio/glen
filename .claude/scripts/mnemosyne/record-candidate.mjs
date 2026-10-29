#!/usr/bin/env node
// Mnemosyne — rule_candidates upsert/merge (for the distiller).
//
// Requires a `rule_findings`/`rule_candidates` table pair in your project's own
// Postgres — see docs/LearnedRulesLoop.md for the schema.
//
// Upserts a distilled candidate rule. Re-running merges new evidence into an
// existing candidate (dedupe on normalized rule_text + category) instead of
// creating duplicates: the supporting_evidence_ids are unioned and the score is
// recomputed as the count of DISTINCT independent episodes (session_id /
// commit_sha) across the merged evidence. Standalone Node ESM via postgres-js
// against `DATABASE_URL` (same connection contract as record-finding.mjs — no
// hardcoded fallback). Only ever touches rows with status='candidate' — never
// silently re-opens a retired/promoted rule.
//
// Usage:
//   DATABASE_URL=postgres://... node record-candidate.mjs --json '{ "rule_text": "...", "category": "...",
//     "supporting_evidence_ids": ["<uuid>", ...] }'
//
// Score model: score = count(distinct independent episodes) across the
// candidate's evidence (so >=3 independent episodes => score >= 3). The
// distiller's decay/promotion sweep adjusts score further over time.
//
// Output: JSON { action: 'inserted'|'merged', id, score } to stdout.

import postgres from 'postgres';

const DB_URL = process.env.DATABASE_URL;
if (!DB_URL) {
  console.error(
    'record-candidate: DATABASE_URL env var is required (see docs/LearnedRulesLoop.md) — no default connection is assumed',
  );
  process.exit(1);
}

const argv = process.argv.slice(2);
const flag = (name) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : undefined;
};

/** Normalize rule_text for the merge-dedupe key: lowercase, collapse spaces. */
function normKey(text) {
  return String(text).toLowerCase().replace(/\s+/g, ' ').trim();
}

/** Count distinct independent episodes (session_id OR commit_sha) over a set
 *  of finding ids. An episode with neither id counts as one isolated episode
 *  (its own row id). */
async function countEpisodes(sql, evidenceIds) {
  if (!evidenceIds || evidenceIds.length === 0) return 0;
  const rows = await sql`
    SELECT id, session_id, commit_sha
    FROM rule_findings
    WHERE id = ANY(${evidenceIds})
  `;
  const keys = new Set();
  for (const r of rows) {
    const k = r.session_id || r.commit_sha || `id:${r.id}`;
    keys.add(k);
  }
  return keys.size;
}

async function main() {
  const jsonArg = flag('--json');
  if (jsonArg === undefined) throw new Error('record-candidate: pass --json <obj>');
  const input = JSON.parse(jsonArg);
  if (!input.rule_text) throw new Error('record-candidate: rule_text is required');
  if (!input.category) throw new Error('record-candidate: category is required');
  const evidenceIds = Array.isArray(input.supporting_evidence_ids)
    ? input.supporting_evidence_ids.map(String)
    : [];

  const key = normKey(input.rule_text);
  const sql = postgres(DB_URL, { max: 1, prepare: false });
  let result;
  try {
    // Find an existing candidate row (status='candidate') with the same
    // normalized rule_text + category.
    const matches = await sql`
      SELECT id, supporting_evidence_ids
      FROM rule_candidates
      WHERE category = ${input.category}
        AND status = 'candidate'
        AND lower(regexp_replace(rule_text, '\\s+', ' ', 'g')) = ${key}
      LIMIT 1
    `;

    if (matches.length > 0) {
      const row = matches[0];
      const merged = Array.from(
        new Set([...(row.supporting_evidence_ids || []), ...evidenceIds]),
      );
      const score = await countEpisodes(sql, merged);
      await sql`
        UPDATE rule_candidates
        SET supporting_evidence_ids = ${merged},
            score = ${score},
            updated_at = now()
        WHERE id = ${row.id}
      `;
      result = { action: 'merged', id: row.id, score };
    } else {
      const score = await countEpisodes(sql, evidenceIds);
      const inserted = await sql`
        INSERT INTO rule_candidates (rule_text, category, supporting_evidence_ids, score, status)
        VALUES (${input.rule_text}, ${input.category}, ${evidenceIds}, ${score}, 'candidate')
        RETURNING id
      `;
      result = { action: 'inserted', id: inserted[0].id, score };
    }
  } finally {
    await sql.end({ timeout: 5 });
  }
  process.stdout.write(`${JSON.stringify(result)}\n`);
}

main().catch((err) => {
  console.error(`record-candidate: ${err.message}`);
  process.exit(1);
});
