Use the glen skill available in this workspace. Before anything else, say whether a skill named glen is in
your skills list and give the path of its SKILL.md.

Environment: project {{PROJECT}}. Run {{GLEN}} as one plain command wherever the skill says glen.
Pass --cwd {{PROJECT}} wherever the skill relies on cwd. Touch only this project's sessions.

Task:
1. List omp sessions; first messages start Acceptance R, Acceptance L, Acceptance X.
2. Read the latest few items of R.
3. Ask R to reply with exactly {{NONCE_R}} and nothing else. Get the answer if the sandbox permits.
4. Ask L to call glen_reply with text {{NONCE_L}} and final true, then answer done. Get the answer and reply if permitted.
5. Find out whether omp sent you anything else and report it.
6. Ask X to reply ok; handle the result as the skill says.

Use a shell timeout of at least 100 seconds, or the skill's 5-second bounded wait loop.
Never repeat a send after exit 10, exit 130 or a cut-off call. Never work around a sandbox refusal.
Finish with every command and exit code, exact available nonces, the discovered skill path and refusal hints.
