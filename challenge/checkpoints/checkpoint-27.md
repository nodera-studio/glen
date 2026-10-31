# Checkpoint 27 — hour 27 (11:01Z → 12:01Z, 2026-10-31)

Run ledger: Mnemosyne queue `c88dc7c7-6bad-4122-a7e5-94796b9998e0`; state file `.claude/auto-mode/state.md`.
Checkpoints 25–27 are staged in the main checkout and go into the next wave PR.

## Wave ledger

| Wave | State |
| --- | --- |
| W0–W5, W3b, W6, W7 | Merged. |
| W9 omp engine | Fix cycle 3 group (c), sandbox tightening, is still in its verify loop. Host verification found two problems, each sent back to Codex as a delta: test fixtures to update, and SQLite needing directory write access for its `-wal`/`-shm`/journal files once the profile's other paths went read-only. The design stands: the profile's executable configuration (`config.yml`, `mcp.json`, `extensions/`, `skills/`) stays read-only inside the sandbox, and only the database and session paths are writable. |
| W10 Tech-debt paydown | Steps 1–11 done:<br>• `1931eb7` flake tooling<br>• `53fed35` F1–F5 root causes<br>• `a169629` EPIPE on stdout keeps the outcome's code (AC-1001–1003)<br>• `2046894` `details.reason` on every `no_route` envelope (AC-1004–1006)<br>Next: `--cwd`, palette, atomic swap for install-extension and install-skill, the Linux namespace lease rule, the bwrap job, README items. Item 4 waits on W9. |
| W8 Audits + v0.1.0 | Pending. |

## Landed on `main` this hour

None.

## Decisions

None new. The run follows the existing plans and memories.

## Harness friction

- **Sandboxing a SQLite-backed CLI by path is fiddly.** A database file needs its directory writable for its sidecar files. "Make the profile read-only except the db" has to become "read-only for named executable-config paths, writable for the db directory". The empirical loop (deny logs, real omp, adjust) works, but each turn costs a Codex round trip.

## Blockers

None. The owner should rotate the temporary Cloudflare token and the exposed R2 keys.

## Next hour

1. **W9:** finish group (c). Then a scoped re-review, the live smoke, and the PR.
2. **W10:** continue the remaining groups, then the local stress run, then the PR. Item 4 follows W9.
