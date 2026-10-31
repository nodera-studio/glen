# W9 cycle-4 Codex scoped re-review — e9b8373..37a52bd (static; sandbox_apply not permitted in Codex's env)

Verdict: REQUEST CHANGES.

Per fixes4 item: 1 partial, 2 partial, 3 closed, 4 partial (no deny logs supplied), 5 partial, 6 closed, 7 closed, 8 closed, 9 partial, 10 partial, 11 partial (no `high` argv assertion), 12 closed, 13 partial, 14 open (no read grant for ~/.omp/run despite write grant at omp-sandboxed.sh:147).

- P1 omp-engine.sh:100-139 — launcher exclusion is lexical, not physical: a cache symlink resolving into a writable profile cache/run dir passes; same-UID owner/mode check can't stop a child rewriting contents (check→exec window). omp-profile-setup.sh:169-174 passes the same dir as launcher destination and sandbox run root. Fix: realpath containment + trusted ancestors; remove setup's child-writable launcher copy.
- P2 omp-sandboxed.sh:56/140 — `"` in a path breaks the `#"..."` regex literal (probe with /fake/a"b/agent → exit 65). Fix: ordinary escaped SBPL string or regex-quote; tests for quote/backslash paths.
- P2 omp-engine.sh:341 — nocaseglob has no effect on literal patterns AGENTS.md/CLAUDE.md; cLaUdE.mD missed on case-sensitive FS (smoke 639-645). Fix: [A]GENTS.md / [C]LAUDE.md patterns.
- P2 src/commands/send.rs:258 — warning suggests sending "/effort <level>", but bridge sendUserMessage disables command handling (omp agent-session.ts:10302-10313) so it is plain model input. Fix: tell the user to type /effort in the omp TUI, or point at a supported control; test the remedy.
- P2 omp-engine-smoke.sh:479-490, 780-793 — Linux expectations contradict the new refusal. Fix: assert the Linux refusal regardless of bwrap; gate macOS-only dispatch fixtures.
- P2 omp-engine.sh:438-439, 693; model-ladder.sh:20 — explicit --model still gets ladder thinking (`--thinking medium`), contradicting SKILL.md:32-33. Fix: use the persisted suffix; omit the flag when the configured model has none.
