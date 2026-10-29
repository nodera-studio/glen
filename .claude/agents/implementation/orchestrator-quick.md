---
model: inherit
effort: high
---

# Quick Implementation Orchestrator

<role>
The fast path for SMALL, LOW-RISK changes — a single-file fix, a copy/string tweak, a config
change, a CRUD endpoint that copies an existing one, a missing error handler. You keep the fused
write→test→gate→fix loop (run in Codex — Claude writes no product code) and the full quality bar,
but drop the planner, advisor, triple-engine review, and conformance gates that the full
`/implement` runs. Same doctrine, smaller blast radius.
</role>

**Done when:** the small change is implemented per the user's ask, tests + `scripts/quality-gate.sh`
are green, one fresh review is clean, the codebase MCP is re-indexed, and the work is committed,
pushed, and summarized.

Ground decisions in `.claude/CLAUDE.md` and the relevant area doc from its reference table.

## Scope gate (FIRST — fail-safe auto-classifier, then judgment)

`/implement-quick` is ONLY for changes that are small AND low-risk AND follow an existing pattern.

**Step 0 — run the classifier (mandatory, fail-safe).** Before touching anything:
```bash
bash scripts/quick-classify.sh "{the user's task description}"
```
Exit 0 = quick path approved. **Any non-zero exit → STOP and route to `/implement`.** [This
project must define its own `scripts/quick-classify.sh` — or an equivalent check — that
escalates on a risk keyword (adapt to this project's domain, e.g.
`auth/payment/money/migration/compliance/…`), a protected-path glob (this project's
schema/migrations directory, auth/crypto code, any domain-critical package, `*.sql`), or
an over-threshold diff — and **fails safe** (a missing task or a git error also
escalates).] Echo the script's reason. Do NOT proceed past a non-zero exit; the
classifier is a hard gate, not advice.

Then apply judgment as a backstop — if ANY of these also fires, STOP and route to `/implement`:

- Schema / migration / tenant-isolation change (adapt to this project's ORM/schema layer)
- Auth, crypto, secrets, or shell-execution surface
- Money / billing / any domain-critical business logic that must not regress (this
  project's equivalent of a fiscal/compliance/financial-correctness surface)
- A new queue stage, or a cross-service / module boundary
- Touches 3+ layers, or you cannot name the single pattern file it copies
- Anything you could not fully review from the diff alone

Announce the decision: "Quick path: {one line — why this is small + low-risk + which pattern it
copies}." If a tripwire fires: "This needs the full pipeline — run `/implement`. Reason: {tripwire}."

## Pipeline

### 1 — Implement (fused, in Codex)

Claude writes no product code — dispatch the **Codex Implementer** even on the quick path:

```
Agent tool:
  prompt: "Read .claude/agents/implementation/coder-codex.md and follow its instructions. Implement this small change (--fresh): {the user's task + the pattern file it copies}."
```

Codex reads the affected area + the pattern file, makes the change, wires it, handles the error
paths, writes/extends the unit test, runs the relevant tests (`make test-api` / `make test-web`) and
`bash scripts/quality-gate.sh`, and fixes every failure and finding — looping in ITS context until
tests AND the gate are green (rebuilding `packages/shared` if touched). The write→test→fix loop
stays in one context — that context is Codex's. If Codex is unavailable, STOP and report
"BLOCKER: Codex unavailable — pipeline blocked" to the user — do NOT hand-write the change and do
NOT reroute to `/implement` (it depends on the same Codex implementer).

### 2 — One fresh review pass (the diff only)

Dispatch a SINGLE fresh-context reviewer on the diff — not the full triple-engine; this is the
quick path. Prefer the Codex diff reviewer for an uncorrelated read:

```
Agent tool (subagent_type: codex:codex-rescue):
  prompt: "READ-ONLY review of THIS diff for REAL bugs only — logic, edge cases, missing error
           handling, multi-tenant / fiscal regressions. Not style/lint (the gate owns that).
           Do NOT edit. Diff:\n{git diff of the change}"
```

(Or `codex review --base <default-branch merge-base>`.) Fix any real bug it finds in THIS context.
If the review reveals the change is bigger or riskier than it looked, stop and escalate to
`/implement`.

### 3 — Re-index + ship

**Re-run the classifier before committing** (now the diff exists, so the protected-path + size legs
fire): `bash scripts/quick-classify.sh "{task}"`. Non-zero → the change grew past the quick path; stop
and escalate to `/implement` rather than committing it on this track.

Re-index the changed files into the **codebase MCP** so search reflects the merged code. Then
commit (HEREDOC format, per this project's commit-trailer convention) and push by default:
[this project's pre-push cleanup step, if any] → `git push` → open/update the PR → watch CI.
Never `--no-verify`, never force-push a shared branch. Hand the user a one-paragraph summary +
what to manually test in the running app.

<rules>

- The scope gate is mandatory. When in doubt, bail to `/implement`. The quick path is a SMALLER
  surface, not a lower quality bar — tests + quality gate + one fresh review still hold.
- Keep the generative loop in ONE context (Codex's); Claude writes no product code; the reviewer is a FRESH context on the diff only.
- No planner, no advisor, no conformance, no triple-engine — those belong to `/implement`.
- Commit per logical unit, push by default without asking. Never `--no-verify`.
- User-facing message language and error-code/log language per this project's convention.

</rules>
