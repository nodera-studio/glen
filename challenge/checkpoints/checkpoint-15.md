# Checkpoint 15 — hour 15 (15:01Z → 16:01Z, 2026-10-30)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.

## Wave ledger

| Wave | State |
| --- | --- |
| W0, W1, W2, W3, W6, W7 | Merged (PRs #3, #9, #10, #16, #7, #12). Site live at https://glen.nodera.workers.dev |
| W4 Agent surface | Code is on `main`. **Acceptance:** modes **a and b2 pass** on live omp (evidence `w4-acceptance/20261030T1541`). Mode **b** ran end to end: the agent got exact R and L nonces, read the inbox, and X was refused with no retry. Its verdict failed on one compound shell call and on two harness defects that are now being fixed. **b3** was blocked by Claude Code's `dontAsk` denying the Skill loader; the fix is `6fc24a5`. **c1 and c2** come next. The harness fix branch `fix/w4-acceptance-harness` has 4 commits so far. |
| W3b Session controls | In progress. Codex finished steps 1–2, and the resumed run on steps 3–11 is still going (about 1.5 h). No WIP commit yet. |
| W5 `glen watch` | In progress. `49893c6` deps, `a690f21` steps 1–2, `f6ff4ef` step 3 (state, reducer, prompt editor, single keymap table). Step 4 (layout and view) is running. |
| W9 | Pending, after W5. |
| W10 Tech-debt paydown | 13 items (board `de08958c`). New this hour: item 10, `tests/live.rs` flakes (`fake_ext_protocol_table`, `ac316_outcomes`); item 11, exit 1 on a broken pipe (policy); item 12, `no_route` lacks `details.reason` for a live owner; item 13, `tail` rejects `--cwd`. |
| W8 Audits + v0.1.0 | Pending, after W10. |

## Landed this hour

Nothing new on `main`. On `fix/w4-acceptance-harness` (not yet a PR):
- `f7c58ba`: `screen -p 0` and a TUI readiness gate; the b2 checker now parses JSON results; a verdict is written when setup fails early.
- `857b53b`: `evidence_shapes` fails loudly when there is no recording.
- `b0571fa`: no `grep -q` pipelines under `pipefail` (SIGPIPE flake in the W3 gate).
- `6fc24a5`: b3 pre-allows the Skill loader, and AC-420-loaded requires a skill load that was not denied.

## Decisions

- **Acceptance b3 pre-allows only the generic `Skill` loader** (`--allowedTools Skill`); Bash stays un-allowed. b3 still proves that the glen Bash permission comes only from the skill's `allowed-tools`. Orchestrator decision; memory `c83ad5ab` records the Claude Code behaviour.
- **Evidence transcripts drop Claude Code `api-request` telemetry lines**, instead of adding a gitleaks allowlist. Their beta-header lists tripped `generic-api-key` 8 times; I checked by shape that none was a credential.
- Facts recorded:
  - macOS screen 4.00.03 needs `-p 0`, and omp accepts keys in cooked mode until its settings load (`399ccfc1`);
  - SIGPIPE under `pipefail` (`77b94309`).

## Harness friction this hour

- **The acceptance harness had five latent defects** that its offline self-test could not catch:
  - missing `screen -p 0`;
  - typing before omp was ready;
  - the b2 regex;
  - the `grep -q` SIGPIPE;
  - `dontAsk` blocking the Skill loader.

  It took three live attempts to surface them all. Lesson for batch 9: a harness that drives real TUIs needs one live smoke run during its own wave, not only at acceptance time.
- The W3 plan's manual steps never recorded the `-p 0` workaround that the orchestrator used. Plan text was copied into code, bug included.
- W3b's long resume run (~1.5 h) has no WIP commit yet. The batch-8 rule assumes runs are short.

## Blockers

None. The owner still needs to add the `CLOUDFLARE_API_TOKEN` repo secret for CI deploys.

## Next hour

1. Land the harness fixes (evidence_shapes accepts error envelopes; the transcript filter). Re-run mode b once, which uses up its one allowed model-nondeterminism rerun. Then run b3, c1, and c2 (config backup, temporary network, byte-compare restore). Open the W4 acceptance PR with the evidence README.
2. W3b: finish steps 3–11, then the review gate.
3. W5: steps 4 onward. Once W3b passes its review gate, merge W3b in and turn on the controls.
4. Harness batch 9: the Implementer must always dispatch Codex; clean up orphaned companion tasks; WIP commits during long runs; a live smoke step for harnesses that drive real TUIs.
