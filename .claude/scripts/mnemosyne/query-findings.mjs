#!/usr/bin/env node
// Mnemosyne — rule_findings window query (read-only, for the distiller).
//
// Requires a `rule_findings`/`rule_candidates` table pair in your project's own
// Postgres — see docs/LearnedRulesLoop.md for the schema.
//
// Dumps the last-N-days rule_findings grouped by category as JSON, so the
// nightly distiller (claude -p) can cluster over them. Read-only SELECT;
// never writes. Standalone Node ESM via postgres-js against `DATABASE_URL`
// (same connection contract as record-finding.mjs — no hardcoded fallback).
//
// Usage:
//   DATABASE_URL=postgres://... node query-findings.mjs --since 7d          # default window = 7 days
//   DATABASE_URL=postgres://... node query-findings.mjs --since 30d --json  # JSON only (no human summary)
//
// Output: JSON to stdout:
//   { since, window_days, total, by_category: { <cat>: [ {row...}, ... ] } }
// A short per-category count summary goes to stderr unless --json is passed.

import postgres from 'postgres';

const DB_URL = process.env.DATABASE_URL;
if (!DB_URL) {
  console.error(
    'query-findings: DATABASE_URL env var is required (see docs/LearnedRulesLoop.md) — no default connection is assumed',
  );
  process.exit(1);
}

const argv = process.argv.slice(2);
const flag = (name, def) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : def;
};
const jsonOnly = argv.includes('--json');

/** Parse a "7d" / "30d" / "168h" window into whole days for the interval. */
function parseDays(since) {
  const m = String(since).match(/^(\d+)\s*([dh])?$/i);
  if (!m) throw new Error(`query-findings: bad --since "${since}" (use e.g. 7d)`);
  const n = parseInt(m[1], 10);
  const unit = (m[2] || 'd').toLowerCase();
  return unit === 'h' ? Math.max(1, Math.ceil(n / 24)) : n;
}

async function main() {
  const since = flag('--since', '7d');
  const days = parseDays(since);

  const sql = postgres(DB_URL, { max: 1, prepare: false });
  let rows;
  try {
    // Interval is parameterized via make_interval to avoid string interpolation.
    rows = await sql`
      SELECT id, source, engine, finding_text, category, file, accepted, action,
             commit_sha, session_id, created_at
      FROM rule_findings
      WHERE created_at >= now() - make_interval(days => ${days})
      ORDER BY category NULLS LAST, created_at DESC
    `;
  } finally {
    await sql.end({ timeout: 5 });
  }

  const byCategory = {};
  for (const r of rows) {
    const key = r.category ?? '(uncategorized)';
    (byCategory[key] ||= []).push(r);
  }

  const out = {
    since,
    window_days: days,
    total: rows.length,
    by_category: byCategory,
  };
  process.stdout.write(`${JSON.stringify(out, null, 2)}\n`);

  if (!jsonOnly) {
    for (const [cat, list] of Object.entries(byCategory)) {
      console.error(`  ${cat}: ${list.length}`);
    }
    console.error(`query-findings: ${rows.length} findings in last ${days}d`);
  }
}

main().catch((err) => {
  console.error(`query-findings: ${err.message}`);
  process.exit(1);
});
