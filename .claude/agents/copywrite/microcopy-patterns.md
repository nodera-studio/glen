# Microcopy Patterns

Pattern catalogue loaded by `microcopy-specialist.md`. Organized by component family.

<!--
This file ships with GENERIC examples so the pattern shapes are clear out of the box.
Replace the illustrative copy with THIS project's actual product vocabulary once you
have real strings to draw from (an entity name like "invoice" below might be "ticket",
"order", "post", or whatever your product's core object is) — but the PATTERNS
(sentence case, verb-leading, banned phrases, past-tense success toasts) are the
reusable part and should stay intact.
-->

---

## 1. Buttons

**Pattern:** Sentence case, verb-leading, three words or fewer. Always start with the action verb.

**Examples:**
- `Save changes`
- `Create invoice`
- `Send to <recipient>`
- `Add customer`
- `Cancel`

**Anti-patterns:**
- `Click here to save` — instructions before the verb (drop "Click here", lead with `Save`)
- `OK` — vague consent; replace with the action it confirms (`Save`, `Delete`, `Send`)
- `Submit Form` — title case + abstract noun (use `Send` or the specific action)
- A noun-form label (e.g. `Saving`) where an imperative verb (`Save`) is correct

---

## 2. Empty states

Four canonical variants. Each variant has one headline + one body line + one primary CTA.

### First-use (the user has never created this resource)

- Headline: `No invoices yet`
- Body: `Create your first invoice to start billing customers.`
- CTA: `Create invoice`

### Filtered-no-match (filters are active, no rows match)

- Headline: `No matches`
- Body: `Try different filters or search terms.`
- CTA: `Clear filters`

### Achievement-zero (the user finished everything, table is intentionally empty)

- Headline: `All clear`
- Body: `No invoices to send today.`
- CTA: (none — celebratory state; CTA optional)

### Error (failed to load the resource)

- Headline: `Something's not working`
- Body: `We couldn't load your invoices. Try again.`
- CTA: `Reload`

---

## 3. Errors

**Pattern:** WHAT failed + WHY (when knowable) + WHAT to do next. Three sentences max; one sentence is fine.

**Examples:**
- `Couldn't send the invoice. The server returned an error. Try again in a minute.`
- (no known why): `Couldn't save your changes. Try again or refresh the page.`

**Banned phrases (always reject):**
- `Oops!`
- `Whoops!`
- `Sorry, something went wrong.`
- `An error occurred.` (vague — what error, what to do?)

These are AI-slop hedges. They tell the user nothing and waste the line.

---

## 4. Toast messages

**Pattern:**
- Success: past-tense verb, max 6 words. The action completed; report it.
- In-flight: present-continuous verb with ellipsis, max 4 words.
- Failure: see § 3 (Errors).

**Examples:**
- Success: `Invoice created.` / `Customer saved.` / `Sent.`
- In-flight: `Creating…` / `Sending…` / `Saving…`

**Anti-patterns:**
- `Your invoice will be created shortly` (future tense — wrong for a confirmation)
- `Invoice was successfully created` (passive + redundant "successfully" — every success is successful)

---

## 5. Form labels and placeholders

**Labels are nouns. Placeholders are examples.** Never put instructions in a placeholder — the label or helper text owns instructions.

**Examples:**
- Label: `Email` — Placeholder: `name@company.com`
- Label: `Phone` — Placeholder: `+1 555 123 4567`
- (Add this project's actual domain-specific fields here, e.g. a tax ID, an account number — with a realistic-looking but obviously-fake example value.)

**Anti-patterns:**
- Placeholder reads `Enter your email address` — instruction; replace with an example
- Placeholder reads `e.g., name@company.com` — drop the `e.g.,` prefix; the placeholder IS the example
- Label reads `Email address` — verbose; use `Email` (the noun the user knows)

**Numeric placeholders preserve their digit form across locales.** A placeholder reading `12345678` does not become spelled-out words — the field expects a digit string, the placeholder shows a digit string. (Locale-specific formatting, e.g. thousand separators or date order, is a legitimate adaptation — that's different from spelling numbers out.)
