#!/usr/bin/env node
// Orchestrator mode — agent-gap recorder.
//
// Appends one gap record to BOTH the machine-readable ledger
// (.claude/orchestrator/gaps.jsonl) and the human-readable index
// (.claude/orchestrator/GapLog.md), so /audit-agent-gaps has something to
// cluster and a person can skim the log without parsing JSON. Plain Node/fs —
// no DB dependency, so this travels cleanly to a portable harness.
//
// Usage:
//   node record-gap.mjs --json '{"task_summary": "...", "closest_existing_agent": "...",
//     "why_no_fit": "...", "adhoc_agent_base": "...", "adhoc_prompt_gist": "...",
//     "tags": ["..."]}'
//
// Required fields: task_summary, why_no_fit, adhoc_agent_base.
// Optional: closest_existing_agent (default "none"), adhoc_prompt_gist, tags (default []).

import { readFileSync, writeFileSync, existsSync, mkdirSync, appendFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { execSync } from 'node:child_process';

const here = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = (() => {
  try {
    return execSync('git rev-parse --show-toplevel', { encoding: 'utf8' }).trim();
  } catch {
    return join(here, '..', '..', '..');
  }
})();
const ORCH_DIR = join(REPO_ROOT, '.claude', 'orchestrator');
const JSONL_PATH = join(ORCH_DIR, 'gaps.jsonl');
const GAPLOG_PATH = join(ORCH_DIR, 'GapLog.md');
const OPEN_TABLE_HEADER =
  '| ID | Date | Task | Closest agent | Why no fit | Ad-hoc agent | Tags |';

const argv = process.argv.slice(2);
const flag = (name) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : undefined;
};

function loadGap() {
  const jsonArg = flag('--json');
  if (jsonArg === undefined) {
    throw new Error('record-gap: pass --json \'<gap-object>\'');
  }
  const parsed = JSON.parse(jsonArg);
  if (!parsed.task_summary) throw new Error('record-gap: task_summary is required');
  if (!parsed.why_no_fit) throw new Error('record-gap: why_no_fit is required');
  if (!parsed.adhoc_agent_base) throw new Error('record-gap: adhoc_agent_base is required');
  return {
    task_summary: String(parsed.task_summary),
    closest_existing_agent: parsed.closest_existing_agent
      ? String(parsed.closest_existing_agent)
      : 'none',
    why_no_fit: String(parsed.why_no_fit),
    adhoc_agent_base: String(parsed.adhoc_agent_base),
    adhoc_prompt_gist: parsed.adhoc_prompt_gist ? String(parsed.adhoc_prompt_gist) : null,
    tags: Array.isArray(parsed.tags) ? parsed.tags.map(String) : [],
  };
}

function genId() {
  return `g-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
}

/** Escape pipe characters so a value can't break a markdown table row. */
function cell(text) {
  return String(text).replace(/\|/g, '\\|').replace(/\n/g, ' ').trim();
}

function appendJsonl(record) {
  mkdirSync(ORCH_DIR, { recursive: true });
  appendFileSync(JSONL_PATH, JSON.stringify(record) + '\n');
}

function appendGapLogRow(record) {
  if (!existsSync(GAPLOG_PATH)) {
    throw new Error(
      `record-gap: ${GAPLOG_PATH} is missing — it should exist as a checked-in scaffold`,
    );
  }
  const content = readFileSync(GAPLOG_PATH, 'utf8');
  const lines = content.split('\n');
  const headerIdx = lines.findIndex((l) => l.trim() === OPEN_TABLE_HEADER);
  if (headerIdx < 0) {
    throw new Error(`record-gap: could not find the open-gaps table header in ${GAPLOG_PATH}`);
  }
  // Skip the header row and the separator row (--- | --- | ...) directly under it.
  let insertAt = headerIdx + 2;
  while (insertAt < lines.length && lines[insertAt].trim().startsWith('|')) insertAt++;

  const date = record.ts.slice(0, 10);
  const row =
    `| ${cell(record.id)} | ${date} | ${cell(record.task_summary)} | ` +
    `${cell(record.closest_existing_agent)} | ${cell(record.why_no_fit)} | ` +
    `${cell(record.adhoc_agent_base)} | ${cell(record.tags.join(', '))} |`;

  lines.splice(insertAt, 0, row);
  writeFileSync(GAPLOG_PATH, lines.join('\n'));
}

function main() {
  const gap = loadGap();
  const record = { id: genId(), ts: new Date().toISOString(), status: 'open', ...gap };
  appendJsonl(record);
  appendGapLogRow(record);
  console.log(`record-gap: logged ${record.id}`);
}

main();
