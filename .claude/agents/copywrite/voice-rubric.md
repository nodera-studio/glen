# Voice Rubric (Distilled)

A pocket guide for every specialist except `voice-editor.md`, who loads the full Apple Style Guide. Nine rules. Each rule states the positive imperative, explains why in one sentence, and shows one before/after pair.

---

## 1. Use contractions in English

**Rule:** Contract where natural — `it's`, `we'll`, `you're`, `don't`, `here's`.

**Why:** Contractions read conversationally and signal warmth. Without them, English copy slides into press-release register and stops sounding like a person.

**Before → After**

- `You will receive an invoice shortly.` → `You'll get your invoice shortly.`

---

## 2. Speak directly to the user — second person, informal register

**Rule:** Address the reader as `you`, and in languages with a formal/informal distinction (Romanian `tu`/`dumneavoastră`, German `du`/`Sie`, French `tu`/`vous`), use whichever register your project has committed to. Drop `the user can`, `users are able to`, and the formal pronoun if your register decree says informal.

**Why:** Second person turns documentation into conversation. Peer-tone consumer software earns trust with an informal register; switching to the formal register mid-product makes the tool feel like a government form. Which register is right depends entirely on your market and brand — decide it once (see `localization-register-memo.md.template`) and defend it consistently.

**Before → After**

- `The user can export invoices.` → `You can export invoices.`

---

## 3. Lead with the verb the user is about to do

**Rule:** Open buttons, links, and primary CTAs with the action verb. `Create invoice`, not `New invoice`. `Submit report`, not `Report submission`.

**Why:** A verb-first label tells the reader the consequence of the click before they parse the noun. Noun-first labels read like categories, not commands.

**Before → After**

- `New customer` → `Add customer`
- `Changes` → `Save changes`

---

## 4. Cut filler

**Rule:** Replace ceremonial phrases with their core verbs. `In order to` → `to`. `At this time` → `now`. `Please be advised that` → drop entirely. Every language has its own ceremonial-politeness padding (Romanian `vă rugăm să`, French `veuillez`, formal-register imperatives generally) — cut it the same way.

**Why:** Filler buys nothing and adds reading time. The reader is here to act, not to be addressed formally.

**Before → After**

- `In order to continue, please click the button below.` → `Continue below.`
- `Please make sure to fill in the required fields.` → `Fill in the required fields.`

---

## 5. One idea per sentence

**Rule:** When a sentence contains two ideas joined by `and`, `which`, or a comma splice, split it.

**Why:** A reader who reaches the second clause has already forgotten the first. Splitting preserves both ideas at full strength.

**Before → After**

- `We've created your invoice and you can download it from the invoices tab where it'll stay archived indefinitely.` → `Your invoice is ready. Download it from the Invoices tab. We keep it archived for you.`

---

## 6. Prefer concrete nouns

**Rule:** Use the user's word for the object. `Invoice`, not `document`. `Customer`, not `entity`. Whatever your product's domain calls the thing on screen, use that word — not a generic abstraction one level up.

**Why:** Abstract nouns describe categories; concrete nouns describe the thing on screen. The user is looking at an invoice, a shipment, a ticket — call it that.

**Before → After**

- `Manage the relevant document items in this section.` → `Edit invoice lines here.`

---

## 7. Numbers as digits in microcopy and at ≥10 elsewhere

**Rule:** Microcopy (buttons, labels, toasts, errors) always uses digits. Body copy spells out `one` through `nine` and switches to digits at `10`.

**Why:** Digits scan instantly; spelled numbers read as prose. Microcopy is glanced, not read, so digits win there universally. In body copy, mid-sentence digits like `3` for `three` look unbalanced.

**Before → After**

- Microcopy: `Three invoices selected` → `3 invoices selected`
- Body: `You can attach up to 5 files.` → `You can attach up to five files.`

---

## 8. Sentence case for buttons and headings — title case is forbidden

**Rule:** Capitalize only the first word (and proper nouns). `Submit report`, not `Submit Report`. `Create invoice`, not `Create Invoice`.

**Why:** Sentence case reads naturally and treats the UI as continuous prose. Title case is a publication convention that signals "this is a header" — but in product UI every button looks the same, so the cue carries no information and just adds visual weight.

**Before → After**

- `Save Changes` → `Save changes`
- `Submit To Review` → `Submit to review`

---

## 9. Never use em dashes — and don't lead with AI

**Rule:** No em dashes (`—`) anywhere. Rewrite the sentence with a period, a colon, a comma, or parentheses. The same goes for en dashes (`–`) used as sentence punctuation. Split the clause or restructure it — never reach for a dash to glue two thoughts together.

**Why:** The em dash is the signature of machine-written copy — many house styles ban it on sight so copy never reads as AI-generated. Separately, if AI is a feature rather than the pitch, avoid leading a headline, tagline, or section opener with "AI-native", "AI-powered", "AI-first", or similar. Name the concrete thing the product does; mention AI only where it earns its place as the assist. (This whole rule is a common but not universal house-style choice — drop it if your project's brand voice disagrees.)

**Before → After**

- `Scheduling, messaging, and payments — in one place.` → `Scheduling, messaging, and payments, all in one place.`
- `The AI-native platform for growing teams.` → `Scheduling and payments, built for growing teams.`
- `Submit the form — done in a few seconds.` → `Submit the form. It's done in a few seconds.`

---

## Why this exists

The unabridged source is `apple-style-guide.md` (next to this file). The voice-editor loads it in full; every other specialist loads this rubric and skips the guide unless a rare term needs A–Z lookup. If a rule here ever appears to conflict with the guide, the guide wins — the rubric is a working summary, not a replacement.
