---
allowed-tools: Read, Agent, Bash, Grep, Glob
description: Lean code review (1 Claude + 1 Codex, + omp when ready, 2-vote) on the current diff — the /implement Role 2 gate, runnable standalone
argument-hint: [optional scope hint, e.g. "vs main" or a path]
model: inherit
---

Run the lean code-review gate as a standalone pass — the same gate `/implement` runs in
Role 2, but on demand against the current working changes. Claude ‖ Codex always run; the
omp engine (`.claude/skills/omp/SKILL.md`) joins as a third pass whenever its probe is
ready. It stays lean (one pass per engine, no synthesizer); for the exhaustive multi-pass
audit use `/review-implementation` instead.

- **Run it as a workflow** (`.claude/docs/Workflow-Fanout.md`): the engines in parallel,
  then one fresh read-only verifier per must-fix finding, which REPRODUCEs it (re-traces the
  cited `file:line`, or runs the failing case) before the vote. A must-fix that does not
  reproduce drops to a note. Verifiers also check omp-only findings (step 3). "Linux-only code
  unverified: CI run required" is exempt from reproduction: its verifier only confirms that
  the brief gave no passing Linux CI run id for the reviewed head, and the finding stands.
- **Linux-only code waits for CI.** When the diff adds or changes `cfg(target_os = "linux")`,
  `bwrap`, `unshare`, `netns`, `renameat2`, `omp-sandboxed-linux.sh` or a Linux job in
  `.github/workflows/`, wait for the draft PR's Linux CI first (`check (ubuntu-latest)` and
  `linux-sandbox`; `gh pr checks <pr>`), and give the Claude and Codex passes the run id and
  any failing logs. With no passing run, either of them reports "Linux-only code unverified:
  CI run required", which blocks the gate on one vote (step 3). omp gets no CI context.

Optional scope hint: $ARGUMENTS

Steps:

1. Establish the diff to review. Every engine reviews the same working-tree scope,
   untracked files included, because that is what `omp-engine.sh review` captures:
   - Default: uncommitted changes — `git diff --stat HEAD`, `git diff HEAD`, and
     `git ls-files --others --exclude-standard` (omp: `--uncommitted`).
   - If `$ARGUMENTS` names a base (e.g. "vs main"): the merge-base to the working tree —
     `MERGE_BASE=$(git merge-base HEAD <base>)`, `git diff "$MERGE_BASE"` plus the
     untracked files above (omp: `--base <base>`). This covers committed, uncommitted and
     untracked changes; never review `git diff <base>...HEAD` alone, which omits the
     working tree omp sees.
   - A path limits the report, not the capture: give the path to every pass, and drop
     findings outside it.
   - If there are no changes, say so and stop.
   - Probe omp: `bash .claude/scripts/omp-engine.sh probe` (exit 0 = ready; exit 3 =
     skipped, including `GLEN_OMP_ENGINE=off`). On a skip, keep its JSON `.reason` for the
     report's one-line note ("omp skipped: <reason>") and run Claude ‖ Codex only.

2. Dispatch ALL passes in a single message (two Agent tool calls, plus the omp Bash call
   when the probe is ready, in one tool-use block, so they run concurrently):
   - **Claude pass** — `subagent_type` default, prompt: "Read
     `.claude/agents/implementation/code-review.md` and follow its
     instructions. Review these changes: {diff scope + changed file list, untracked
     files included}."
   - **Codex pass** — `subagent_type: "codex:codex-rescue"`, prompt:
     "Independent full-category code review (security, architecture, data flow,
     performance, accessibility, error handling, test coverage) of these
     changes: {diff scope + changed file list, untracked files included; review the
     working tree, not only commits}. Conventions are in CLAUDE.md and
     `.claude/rules/*`. If the diff touches rendered output (`site/`, a TUI,
     terminal rendering), build and run it and look at the output before calling
     it correct: screenshot the built page (`npx playwright screenshot` against
     `npx astro preview --host 127.0.0.1 --port <a random free port of your own>`
     run in the background in `site/` and stopped afterwards, across the full
     length of any animation), or run the CLI under a pty (`script -q /dev/null sh
     -c 'stty cols 60; cargo run -- <args>'`); a code trace alone is not enough,
     and if you cannot run it say the UI is unverified. Against the real ~/.omp
     run only read verbs (list, show, tail); for anything else point
     PI_CODING_AGENT_DIR at tests/fixtures or a scratch dir under $TMPDIR, or use
     the fake omp via GLEN_OMP_BIN. Never send, steer, follow-up, abort or switch
     the model of a live omp session. If the diff adds or changes Linux-only code
     (`cfg(target_os = "linux")`, `bwrap`, `unshare`, `netns`, `renameat2`,
     `omp-sandboxed-linux.sh`, a Linux job in `.github/workflows/`) and this brief gives no
     passing `check (ubuntu-latest)` / `linux-sandbox` run id for the reviewed head, report
     HIGH 'Linux-only code unverified: CI run required' at the Linux-only file:line; a macOS
     trace or the host's Linux clippy does not verify it. Report severity + category + file:line + issue + impact
     - fix + confidence. Read-only — report only, do not fix. Do NOT write any
     file: return the full review as your final message text; the orchestrator
     saves it. Run in the foreground: wait for the Codex task to finish and return
     the full review as your final message, never only a background task id.
     --wait --effort high".
   - **omp pass** (only when the probe exited 0) — Bash with `run_in_background: true`,
     from the root of the checkout under review (the wave's worktree inside `/implement`),
     through the engine script only (never `omp` or a bare `glen send`;
     the script always uses `--profile glen-harness`, read-only):
     `mkdir -p target/reports && bash .claude/scripts/omp-engine.sh review --repo "$PWD"
     --uncommitted --out target/reports/omp-review.txt --scope "<scope hint>" >
     target/reports/omp-engine.log 2>&1`. For a named base use `--base <base>` in place of
     `--uncommitted` (step 1's merge-base scope); a path scope goes in `--scope` as text (omp
     still receives the whole diff, so drop its findings outside the path). Read the review in
     `target/reports/omp-review.txt` and the result in `target/reports/omp-review.txt.json`.
     omp traces code only: it cannot run UI or tests, so it never counts as having run them.
     Do not re-run it by hand (the engine retries capacity once itself). A non-zero exit,
     or `"status"` other than `ok` (including `"failed"` with `"reason":"malformed
     output"`), means omp did not run: name its `.reason` in the report and continue with
     Claude ‖ Codex. omp never blocks the gate, and its own 15-minute `--deadline` does not
     bound the whole pass (the snapshot capture runs before that timer starts, the wait
     allows 75 s more, and a capacity retry adds a minute). So note the launch time
     (`date +%s`) and set an absolute gate deadline of launch + 20 minutes. Poll the
     background task (its completion notification, or `target/reports/omp-review.txt.json`
     reaching a terminal `.status`) against that deadline. When it expires with omp still
     running, merge without omp and report "omp: no result by the gate deadline". Leave the
     run alone: read its result later for the record, outside the gate, and never let a late
     result change a verdict already given. Recovering a killed run is in
     `.claude/skills/omp/SKILL.md`.
   - Never name an output path in the Codex brief: its sandbox is workspace-write,
     does not escalate, and writes only inside its own cwd (your shell's directory
     at dispatch, often a worktree), so a write elsewhere fails. Save the returned
     text yourself if it must be on disk.
   - The `--wait` is load-bearing: without it the forwarder may background a long
     review and return at once with a task id. If a task id comes back anyway,
     fetch the review (`codex-companion.mjs status <id> --wait`, then `result <id>`)
     before merging; never merge a pass you have not read.
   - If `codex:codex-rescue` is unavailable, fall back per
     `.claude/skills/codex/SKILL.md`; if Codex is unreachable, run the Claude
     pass (and omp, when ready) without it and note the degraded gate.
   - If the Codex pass fails on capacity or rate ("Selected model is at
     capacity", 429, rate limit), retry it once after a short wait (about 60 s).
     If the retry fails too, record Codex as unavailable and merge the Claude pass
     (with omp, when it ran) without it, saying so in the report. Never drop an
     engine silently.
   - If you save a pass to a file, name it `claude-review.md` / `codex-review.md` /
     `omp-review.txt`, never `claude.md`: on macOS's case-insensitive filesystem
     Claude Code loads any `claude.md` as a nested `CLAUDE.md`, so the review
     would be injected as project instructions. This holds for every way of
     creating the file, including shell redirects and `mv` (the guard hook covers
     the Write tool only).

3. Merge by 2-vote consensus and present ONE report. A vote is one engine raising the same
   defect (same file, overlapping lines, same root issue); omp votes only when its status
   is `ok`.
   - Two or more engines raise it (Claude + Codex, or omp + either) → **HIGH — must fix**.
   - CRITICAL from Claude or Codex → **must fix** (survives a single vote).
   - "Linux-only code unverified: CI run required" from Claude or Codex → **must fix** on one
     vote; it never needs a second engine. It is settled by a passing Linux CI run on the
     reviewed head, not by a code fix, so it is not a fix-cycle item.
   - **omp-only findings are advisory.** omp is a peer engine without a shell or tests, so a
     finding only omp raised carries no vote of its own. Hand each omp-only CRITICAL or HIGH
     to a fresh read-only verifier, which re-traces the cited `file:line` against the code.
     Verified: an omp-only CRITICAL → **must fix**, an omp-only HIGH → recommended fix.
     Not verified: a note saying so. omp-only MEDIUM/LOW → note only.
   - HIGH from Claude or Codex alone → judgement call; list as a recommended fix.
   - MEDIUM/LOW single-engine → note only (do not block).
     Group by must-fix / recommended / notes, each with file:line, the suggested change and
     the engines that raised it. Open the report with one engines line: `Claude ✓/– · Codex
     ✓/– · omp ✓ <model> / – <reason>`.

This command does NOT modify code or commit — it reports. To apply fixes, act on
the report yourself or run `/implement` (whose Step 14 rework loop automates the
fix cycle).
