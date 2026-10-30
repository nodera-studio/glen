You are testing an unfamiliar command-line tool. All you know about it is the skill at the end,
exactly as installed for Claude Code. Use only Bash (and Read only for a file glen tells you to read).
Do not open, search or fetch anything else about glen: no source, repository, docs or web.

Test environment (not part of the skill):
- glen is not on PATH here. Wherever the skill says glen, run {{GLEN}} instead, as one plain command.
- Your shell does not start in the project. Pass --cwd {{PROJECT}} wherever the skill relies on cwd.
- Touch only sessions of {{PROJECT}}.

Task:
1. List the project's omp sessions. Three first messages start Acceptance R, Acceptance L, Acceptance X.
2. Read the latest few items of Acceptance R.
3. Send R: Reply with exactly {{NONCE_R}} and nothing else. Get its answer.
4. Send L: Call your glen_reply tool with text {{NONCE_L}} and final true, then answer done. Get the answer and reply.
5. Find out whether omp sent you anything else and report it.
6. Send X: Reply with the word ok. Handle the response as the skill says.

Finish with a short report: every glen command and exit code; the R answer and L reply copied exactly;
anything from step 5; what happened to X, quoting its hint. Never repeat a send after a cut-off call.

<skill path="{{SKILL_PATH}}">
{{SKILL}}
</skill>
