#!/usr/bin/env bash
# Mnemosyne — weekly promotion digest (cron, e.g. Monday 09:00).
#
# Requires a `rule_findings`/`rule_candidates` table pair in your project's own
# Postgres — see docs/LearnedRulesLoop.md for the schema.
#
# Renders a Markdown digest of candidate rules eligible for promotion
# (status='candidate' AND score>=3), plus the eviction candidate if the active
# promoted set is at/over cap. Each row carries a ready-to-paste promote.mjs
# command — "one click" = one paste. No LLM needed to format a table, so this is
# a plain node render (the tier recommendation is a heuristic; the operator makes
# the final tier call). See digest-prompt.md for the (optional) LLM tier-judgment
# variant.
#
# Cron (box-cron):
#   0 9 * * 1  cd /path/to/your/project && DATABASE_URL=postgres://... bash \
#     .claude/scripts/mnemosyne/weekly-digest.sh >> .claude/.mnemosyne/cron.log 2>&1
set -uo pipefail

if [ -z "${DATABASE_URL:-}" ]; then
  echo "weekly-digest: DATABASE_URL env var is required (see docs/LearnedRulesLoop.md)" >&2
  exit 1
fi

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$REPO_ROOT" || exit 1

ARTIFACT_DIR="$REPO_ROOT/.claude/.mnemosyne"
mkdir -p "$ARTIFACT_DIR"
OUT="$ARTIFACT_DIR/digest-$(date +%F).md"
CAP="${MNEMOSYNE_CAP:-30}"

DATABASE_URL="$DATABASE_URL" \
MNEMOSYNE_CAP="$CAP" \
node - "$OUT" <<'NODE'
import postgres from 'postgres';
import { writeFileSync } from 'node:fs';

const outPath = process.argv[2];
const cap = parseInt(process.env.MNEMOSYNE_CAP || '30', 10);
const sql = postgres(process.env.DATABASE_URL, { max: 1, prepare: false });

/** Heuristic tier hint: subsystem-scoped categories -> memory_mcp; broad -> claude_md.
 *  Customize the `broad` list below for your own project's cross-cutting categories. */
function tierHint(category) {
  const broad = ['rls-tenant', 'audit-registry', 'i18n', 'error-handling'];
  return broad.includes(category) ? 'claude_md' : 'memory_mcp';
}

try {
  const eligible = await sql`
    SELECT id, rule_text, category, score, supporting_evidence_ids, created_at
    FROM rule_candidates
    WHERE status = 'candidate' AND score >= 3
    ORDER BY score DESC, created_at ASC
  `;
  const promotedCount = (
    await sql`SELECT count(*)::int AS n FROM rule_candidates WHERE status = 'promoted'`
  )[0].n;
  const evictionCandidate =
    promotedCount >= cap
      ? (
          await sql`
            SELECT id, rule_text, category, score FROM rule_candidates
            WHERE status = 'promoted' ORDER BY score ASC, updated_at ASC LIMIT 1
          `
        )[0]
      : null;

  const lines = [];
  lines.push(`# Mnemosyne weekly digest — ${new Date().toISOString().slice(0, 10)}`);
  lines.push('');
  lines.push(`Active promoted rules: **${promotedCount} / ${cap}**.`);
  lines.push('');
  if (eligible.length === 0) {
    lines.push('_No candidates eligible for promotion this week (need status=candidate, score≥3)._');
  } else {
    lines.push('## Eligible for promotion (your one-click = one paste)');
    lines.push('');
    for (const c of eligible) {
      const n = Array.isArray(c.supporting_evidence_ids) ? c.supporting_evidence_ids.length : 0;
      const hint = tierHint(c.category);
      lines.push(`### \`${c.category}\` · score ${c.score} · ${n} evidence`);
      lines.push(`> ${c.rule_text}`);
      lines.push('');
      lines.push('```bash');
      lines.push(`node .claude/scripts/mnemosyne/promote.mjs ${c.id} --tier ${hint}`);
      lines.push('```');
      lines.push(`_(tier hint: \`${hint}\` — override to the other tier if you disagree)_`);
      lines.push('');
    }
  }
  if (evictionCandidate) {
    lines.push('## ⚠ Cap reached — promoting one evicts the lowest-scoring rule');
    lines.push('');
    lines.push(
      `Lowest-scoring promoted: \`${evictionCandidate.category}\` (score ${evictionCandidate.score}) — ` +
        `_"${evictionCandidate.rule_text}"_. A new promotion auto-retires it. To retire manually:`,
    );
    lines.push('');
    lines.push('```bash');
    lines.push(`node .claude/scripts/mnemosyne/retire.mjs ${evictionCandidate.id}`);
    lines.push('```');
    lines.push('');
  }

  writeFileSync(outPath, lines.join('\n') + '\n');
  console.log(`weekly-digest: wrote ${outPath} (${eligible.length} eligible, ${promotedCount}/${cap} promoted)`);
} finally {
  await sql.end({ timeout: 5 });
}
NODE

echo "weekly-digest: open $OUT to review + paste a promote command."
