---
allowed-tools: Read, Write, Edit, Bash, Grep, Glob, Agent
description: Polish user-facing copy to Apple voice — report mode (default) or `--apply` to land edits in place
argument-hint: [file|dir|"text"] [--apply] [--lang <locale>] [--surface app|email|docs] [--register marketing|product|email]
model: inherit
---

Invoke the copywriting orchestrator at `.claude/agents/copywrite/orchestrator.md` and follow its pipeline exactly. Pass `$ARGUMENTS` verbatim — the orchestrator parses positional and flag arguments itself.

## Argument parsing

- Positional (one of): file path, directory path, or quoted raw text. The last positional wins if more than one is given. No positional = fall back to text-bearing files in `git diff HEAD`.
- `--apply` — switch to in-place edit mode. Default is report mode.
- `--lang <locale>` — restrict to one locale (e.g. `en`, or whatever locale codes your project ships). Default is all configured locales.
- `--surface app|email|docs` — restrict by surface. Drop or rename surfaces to match your project; exclude any surface intentionally out of scope for this pipeline and note why in your project's copy conventions doc.
- `--register marketing|product|email` — voice register hint passed to the voice-editor and localization-editor. Default is `product`.

## Inline ICU helper — `isStructurallySame()`

The orchestrator's pre-write gate 1 runs this check on any `old_string` matching `/\{[^}]+,\s*(plural|select|selectordinal)/`. The helper is bundled inline because `.claude/` has no package.json scope of its own. The transitive dependency `@formatjs/icu-messageformat-parser` should resolve through whichever workspace in your project already depends on `next-intl` (or an equivalent ICU-based i18n library) — set `{{I18N_WORKSPACE}}` below to that workspace's package filter.

Implementation reference (the orchestrator calls this as `pnpm --filter {{I18N_WORKSPACE}} exec node -e "..."`):

```js
// Safe-edit gate for ICU plural / select strings.
// Returns { safe: true } when only copy text changed.
// Returns { safe: false, reason } when the binding shape changed.
const { parse, isStructurallySame } = require('@formatjs/icu-messageformat-parser');

function safeIcuEdit(oldString, newString) {
  if (!/\{[^}]+,\s*(plural|select|selectordinal)/.test(oldString)) {
    return { safe: true, reason: 'no ICU plural/select in source' };
  }
  try {
    const oldAst = parse(oldString, { requiresOtherClause: true });
    const newAst = parse(newString, { requiresOtherClause: true });
    const result = isStructurallySame(oldAst, newAst);
    return { safe: result.success === true, reason: result.error?.message };
  } catch (err) {
    return { safe: false, reason: `Parse failure: ${err.message}` };
  }
}
```

Runnable form for the orchestrator's pre-write gate — pass the old and new strings via env vars to avoid shell-quoting issues:

```bash
OLD="$old_string" NEW="$new_string" pnpm --filter {{I18N_WORKSPACE}} exec node -e '
  const { parse, isStructurallySame } = require("@formatjs/icu-messageformat-parser");
  const old = process.env.OLD, neu = process.env.NEW;
  if (!/\{[^}]+,\s*(plural|select|selectordinal)/.test(old)) {
    console.log(JSON.stringify({ safe: true })); process.exit(0);
  }
  try {
    const r = isStructurallySame(
      parse(old, { requiresOtherClause: true }),
      parse(neu, { requiresOtherClause: true }),
    );
    console.log(JSON.stringify({ safe: r.success === true, reason: r.error?.message }));
  } catch (err) {
    console.log(JSON.stringify({ safe: false, reason: `Parse failure: ${err.message}` }));
  }
'
```

If your project's primary i18n workspace is unavailable (e.g. running against an email-templates package only), fall back to `pnpm --filter {{EMAIL_WORKSPACE}} exec node -e "..."` with the same body — the parser should resolve through that package's transitive deps as well. If your project doesn't use ICU message formatting at all, skip this gate entirely.

## Inline Babel re-parse — TSX/JSX/MDX safety

The orchestrator's pre-write gate 2 runs after every Edit on a `.tsx`, `.jsx`, or `.mdx` file. It re-parses the post-edit file to confirm no syntax error was introduced. Never call `@babel/generator` — parse-only.

```bash
FILE="$file_path" pnpm --filter {{FRONTEND_WORKSPACE}} exec node -e '
  const fs = require("fs");
  const parser = require("@babel/parser");
  try {
    parser.parse(fs.readFileSync(process.env.FILE, "utf8"), {
      sourceType: "module",
      plugins: ["typescript", "jsx"],
      errorRecovery: false,
    });
    console.log(JSON.stringify({ ok: true }));
  } catch (err) {
    console.log(JSON.stringify({ ok: false, reason: err.message }));
    process.exit(1);
  }
'
```

For files inside a separate templates package (e.g. transactional email templates), swap the workspace filter:

```bash
pnpm --filter {{EMAIL_WORKSPACE}} exec node -e '...'
```

On parse failure, revert the edit via `git checkout -- "$file_path"` (or call the Edit tool again in reverse) and record the rejection in the report under "Rejected by gates". If your frontend doesn't use TSX/JSX/MDX, skip this gate.

## Diacritic / script-character regex — post-pass survival

If your locales use non-ASCII script characters that are easy to silently drop during editing (e.g. Romanian `ăâîșțĂÂÎȘȚ`, or another language's diacritics/accents), define the character class for your project and have the orchestrator count occurrences in the source and in the post-edit file, aborting the file's edits if the count strictly decreased.

```bash
# Count script characters in a file — replace the class with your project's
grep -o '[{{SCRIPT_CHAR_CLASS}}]' "$file" | wc -l
```

If `post_count < source_count`, revert via `git checkout -- "$file"` and write a `required`-severity entry to the report. If none of your locales need this gate, skip it.

## Examples

```bash
# Report mode on a locale message file (default — no edits land)
/copywrite {{I18N_MESSAGES_PATH}}/en.json --lang en --surface app

# Apply mode on an email template (ICU gate + Babel gate + diacritic gate all run)
/copywrite {{EMAIL_TEMPLATES_PATH}}/trial-start.tsx --apply --surface email --register email

# Raw text mode — polish a string inline without touching files
/copywrite "Please press the button to continue." --lang en --surface app

# No positional — fall back to text-bearing files in `git diff HEAD`
/copywrite --surface app
```

See `.claude/agents/copywrite/README.md` for the suite overview and the pipeline diagram.
