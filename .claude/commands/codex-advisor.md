---
allowed-tools: Agent, Read, Glob, Grep
description: Ask the Codex advisor (different-model, read-only) for an adversarial second opinion
argument-hint: <question / plan / decision to pressure-test>
model: inherit
---

# /codex-advisor — different-model second opinion (Codex, read-only)

You have a standing advisor: a DIFFERENT model family (Codex / GPT-5.x) reachable any time
you are unsure, weighing a tradeoff, or about to commit to something hard to reverse. Its
value is **uncorrelated blind spots** — it catches what a second Claude pass cannot, because
same-model self-review samples the same distribution that produced the error.

Question / material: $ARGUMENTS

## Steps

1. **Brief tightly.** Assemble the decision / plan / diff + the constraints that matter. Do
   not dump the whole repo — give the advisor exactly what it needs to judge.
2. **Call it** through the Codex plugin's **general-purpose agent** — this is open-ended
   critique of a plan/design, so use `codex:codex-rescue`, NOT `codex review` (a diff
   reviewer); and NEVER `codex exec`:
   ```
   Agent tool:
     subagent_type: "codex:codex-rescue"
     description: "Advisor — adversarial second opinion"
     prompt: "READ-ONLY adversarial second opinion — advise, do NOT implement or edit
              anything. You are a senior staff engineer, a DIFFERENT model with uncorrelated
              blind spots, NOT the author. Disagree on substance, not style; no praise, no
              filler. If sound, say so with your confidence; if risky, RANK the risks
              worst-first and STEELMAN the alternative you would choose. State what you
              CANNOT assess from the material. End with VERDICT: endorse |
              endorse-with-changes | reject. Material: {brief}. Constraints: {constraints}."
   ```
3. **Synthesize — never just forward.** Read its answer, judge it, report back: the question
   asked + Codex's verbatim verdict + YOUR synthesis (agree / disagree / what to actually
   do). On disagreement, do ONE reconcile call ("I found X, you suggest Y — which constraint
   breaks the tie?") before deciding. Escalate unresolved CRITICAL risks to the user.
4. **Gate the call rate.** Use it for high-value, hard-to-reverse, or low-confidence
   decisions — not naming/style. Over-calling adds noise, not signal.

If Codex is unavailable (`codex login` needed / rate-limited), say so and proceed on your
own judgment — fail-warn, never fail-stop.
