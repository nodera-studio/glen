#!/usr/bin/env node
// Mnemosyne — one-click promotion executor.
//
// Requires a `rule_findings`/`rule_candidates` table pair in your project's own
// Postgres — see docs/LearnedRulesLoop.md for the schema.
//
// "One click" = one paste of this command from the weekly digest. Promotes a
// candidate rule: flips status -> 'promoted', sets tier / promoted_ref /
// reviewed_by / reviewed_at, lands the rule into its tier, and evicts the
// lowest-scoring promoted rule if the active set would exceed the cap.
//
// NEVER auto-runs — a human pastes it deliberately (the supervised gate).
//
// Tiers:
//   --tier claude_md   Appends a one-liner under the bounded
//                      "## Learned review conventions (Mnemosyne)" section of
//                      .claude/CLAUDE.md (hard ~15-line cap), records
//                      promoted_ref = an anchor slug. Fully handled here.
//   --tier memory_mcp  Sets status/tier/audit columns and records
//                      promoted_ref = the entity id you pass via --ref, OR (if
//                      omitted) prints the memory_store payload to paste into a
//                      Claude session, then you re-run with --ref <entity-id>.
//                      A plain node script cannot call the memory MCP itself.
//
// Cap: CAP active promoted rules (default 30). On a promotion that would exceed
// the cap, the lowest-scoring promoted rule is auto-retired (status ->
// 'retired'); its claude_md line is removed if it lived there.
//
// Usage:
//   DATABASE_URL=postgres://... node promote.mjs <candidate_id> --tier claude_md|memory_mcp [--ref <id>]
//                    [--by <name>] [--cap 30]

import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { execSync } from 'node:child_process';
import postgres from 'postgres';

const DB_URL = process.env.DATABASE_URL;
if (!DB_URL) {
  console.error(
    'promote: DATABASE_URL env var is required (see docs/LearnedRulesLoop.md) — no default connection is assumed',
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
const SECTION_HEADER = '## Learned review conventions (Mnemosyne)';
const CLAUDE_MD_LINE_CAP = 15;

const argv = process.argv.slice(2);
const candidateId = argv.find((a) => !a.startsWith('--'));
const flag = (name, def) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : def;
};
const tier = flag('--tier');
const ref = flag('--ref');
const by = flag('--by', 'operator');
const cap = parseInt(flag('--cap', '30'), 10);

if (!candidateId) {
  console.error('promote: usage: node promote.mjs <candidate_id> --tier claude_md|memory_mcp');
  process.exit(1);
}
if (tier !== 'claude_md' && tier !== 'memory_mcp') {
  console.error('promote: --tier must be claude_md or memory_mcp');
  process.exit(1);
}

/** Slug for the CLAUDE.md anchor / promoted_ref. */
function slug(text) {
  return String(text)
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 60);
}

/** Count the rule lines currently under the Mnemosyne section in CLAUDE.md. */
function readSectionLines(content) {
  const lines = content.split('\n');
  const start = lines.findIndex((l) => l.trim() === SECTION_HEADER);
  if (start < 0) return { lines, start: -1, ruleLines: [] };
  const ruleLines = [];
  for (let i = start + 1; i < lines.length; i++) {
    if (lines[i].startsWith('## ')) break;
    if (lines[i].trim().startsWith('- ')) ruleLines.push(i);
  }
  return { lines, start, ruleLines };
}

/** Append a rule line under the Mnemosyne section, creating it if absent. */
function appendClaudeMdLine(ruleText, anchor) {
  let content = existsSync(CLAUDE_MD) ? readFileSync(CLAUDE_MD, 'utf8') : '';
  const line = `- ${ruleText} <!-- mnemosyne:${anchor} -->`;
  if (!content.includes(SECTION_HEADER)) {
    const preamble =
      `\n\n${SECTION_HEADER}\n\n` +
      `> Auto-maintained by \`.claude/scripts/mnemosyne/promote.mjs\` — do not hand-edit.\n` +
      `> Hard cap ${CLAUDE_MD_LINE_CAP} lines; lowest-value rules decay out (see docs/LearnedRulesLoop.md).\n\n`;
    content = `${content.replace(/\s*$/, '')}${preamble}${line}\n`;
    writeFileSync(CLAUDE_MD, content);
    return;
  }
  const { lines, ruleLines } = readSectionLines(content);
  if (ruleLines.length >= CLAUDE_MD_LINE_CAP) {
    throw new Error(
      `promote: CLAUDE.md Mnemosyne section is at the ${CLAUDE_MD_LINE_CAP}-line cap — ` +
        `retire a tier-1 rule first or promote to memory_mcp.`,
    );
  }
  const insertAt = ruleLines.length ? ruleLines[ruleLines.length - 1] + 1 : lines.length;
  lines.splice(insertAt, 0, line);
  writeFileSync(CLAUDE_MD, lines.join('\n'));
}

/** Remove a rule line by its anchor marker (used on cap eviction). */
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
      SELECT id, rule_text, category, status, tier, promoted_ref
      FROM rule_candidates WHERE id = ${candidateId} LIMIT 1
    `;
    if (rows.length === 0) throw new Error(`promote: candidate ${candidateId} not found`);
    const cand = rows[0];
    if (cand.status === 'promoted') throw new Error('promote: candidate is already promoted');
    if (cand.status === 'retired') throw new Error('promote: candidate is retired — re-distill it');

    // memory_mcp without a --ref: print the payload to store, then stop.
    if (tier === 'memory_mcp' && !ref) {
      const payload = {
        text: cand.rule_text,
        tags: ['mnemosyne', 'learned-rule', cand.category],
      };
      console.log('promote: memory_mcp tier needs a memory entity id.');
      console.log('Run this in a Claude session, then re-run promote with --ref <entity-id>:');
      console.log(`  mcp__memory__memory_store ${JSON.stringify(payload)}`);
      return;
    }

    const anchor = `${cand.category}-${slug(cand.rule_text)}`;
    const promotedRef = tier === 'claude_md' ? anchor : ref;

    // Land into the tier BEFORE the DB flip so a tier-landing failure (e.g.
    // CLAUDE.md cap) aborts without a half-applied promotion.
    if (tier === 'claude_md') appendClaudeMdLine(cand.rule_text, anchor);

    await sql`
      UPDATE rule_candidates
      SET status = 'promoted', tier = ${tier}, promoted_ref = ${promotedRef},
          reviewed_by = ${by}, reviewed_at = now(), last_fired_at = now(),
          updated_at = now()
      WHERE id = ${cand.id}
    `;
    console.log(`promote: ${cand.id} -> promoted (tier=${tier}, ref=${promotedRef})`);

    // Cap eviction: if we now exceed the cap, retire the lowest-scoring
    // promoted rule (excluding the one just promoted).
    const promoted = await sql`
      SELECT id, score, tier, promoted_ref FROM rule_candidates
      WHERE status = 'promoted' ORDER BY score ASC, updated_at ASC
    `;
    if (promoted.length > cap) {
      const victim = promoted.find((p) => p.id !== cand.id) ?? promoted[0];
      if (victim.tier === 'claude_md' && victim.promoted_ref) {
        removeClaudeMdLine(victim.promoted_ref);
      }
      await sql`
        UPDATE rule_candidates
        SET status = 'retired', updated_at = now() WHERE id = ${victim.id}
      `;
      const note =
        victim.tier === 'memory_mcp'
          ? ` (memory_mcp entity ${victim.promoted_ref} — call memory_delete on it)`
          : '';
      console.log(`promote: cap ${cap} exceeded — retired lowest-scoring ${victim.id}${note}`);
    }
  } finally {
    await sql.end({ timeout: 5 });
  }
}

main().catch((err) => {
  console.error(err.message);
  process.exit(1);
});
