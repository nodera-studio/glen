# glen site

Static Astro 7, React 19 islands and Tailwind v4. Use Node from `.nvmrc` (24.22.1).

```sh
npm ci
npm run dev
npm run verify       # check, static build, unit tests, budgets, offline link check
npx playwright install chromium  # once
npm run test:e2e     # Chromium smoke; preview starts automatically on 4321 (GLEN_E2E_PORT=<port> for another)
npm run lhci         # mobile, 3 runs per light/dark URL: performance >= 0.90, accessibility >= 0.95
```

For local LHCI with Playwright Chromium, set `CHROME_PATH` to
`node -e "console.log(require('@playwright/test').chromium.executablePath())"`'s output.
`dist/` contains `index.html` and `404.html`; there is no server adapter.

The pre-paint theme bootstrap sets `data-theme` and the `#theme-dark` style's media.
System follows OS dark mode; explicit Light/Dark choices persist in guarded storage.
`?theme=light|dark|system` overrides only the current view and never stores a value.
Without JS, OS theme works, the terminal transcript is complete and all tab panels show.

The hero terminal is server-rendered at its final frame: that is the no-JS, crawler and
reduced-motion view. `src/components/terminal/terminal-player.mjs` holds `plan()` (run at
build to stamp each row with `data-at`/`data-k`/`data-p`) and `boot()`, which esbuild minifies
into an inline script that plays the rows from one rAF clock once the window is in view.
Pause, Replay and the pane tabs are real buttons; leaving the viewport pauses playback.
Test hooks: `?term=end` (final frame), `?term=at:<ms>` (seek and pause), `?term=fast` (x20);
the figure carries `data-term` (state) and `data-term-t` (last applied cue).
Dark CLI tokens are read at build from `../src/render/palette.json`, contract-checked by Rust tests.

Morphin snapshots live in `vendor/morphin/`; provenance and adaptations are in `morphin.json`.
Refresh with pinned `@morphin/cli@1.0.6 add <slug> --overwrite --no-install`, always passing
`--registry https://registry.morphin.dev/registry.json --cwd vendor/morphin/<slug>`.
Review the vendor diff, then port changes by hand into `src/components/morphin/`.
Never add into `src/`. Islands use `motion/react` and `IslandRoot`, never `motion.*`.

Budgets: total JS <= 110 KiB gzip; fonts <= 100 KiB raw; HTML <= 50 KiB gzip;
inline scripts <= 8 KiB gzip, of which the terminal player <= 5,632 B gzip.
No external script or modulepreload in the initial HTML.
`npm run budgets` also checks prohibited imports and terminal palette literals.

## Demo

The terminal shows real glen output. `tests/site_demo.rs` runs the glen binary against the
synthetic sessions in `../tests/fixtures/demo` and stores its coloured stdout in
`src/data/demo-capture.json`; `cargo test` fails when the file drifts from the binary.
`src/data/ansi.mjs` decodes it at build with the palette tokens, and throws on any escape it does
not know. Regenerate after changing glen's output or the fixtures:

```sh
npm run demo:refresh   # GLEN_UPDATE_DEMO=1 cargo test --locked --test site_demo
```

Hand-written in `src/data/demo.mjs`: the typed commands, and what glen prints only to a
terminal's stderr, which a capture cannot see (the send spinner, its progress messages and the
`tail -f` diagnostic). `terminal-player.test.mjs` checks those strings against `../src/`.

## Install section

`installView()` in `src/data/install.mjs` decides what the Install tabs show, from
`releaseAvailable` in `src/data/site.ts`. While it is false the page opens on Cargo, the Script
tab says the script ships with v0.1.0, the Binary tab links to the Releases page, and nothing on
the page names the installer URL or `/releases/latest`. It is true since v0.1.0 was published:
the page opens on the Script tab with a copyable `curl … | sh`, and Binary links to the latest
release.
The closing CTA band repeats the command of the tab Install opens on (`defaultCommand()`), with
the same Copy button.
Islands take what they show as props, never by importing `site.ts`, so the installer URL stays
out of their bundles too.

## Links and hosting

`npm run links` (`scripts/check-links.mjs`) checks the built `dist/` and `../README.md` offline:
both pages exist, no unreleased install URL appears, raw and blob/tree URLs into this repo's
`main` and README relative links resolve in the checkout. `--online` also fetches every other
absolute URL (GET, redirects followed, 3 attempts) and fails on a status of 400 or more.

The site is one assets-only Cloudflare Worker, `glen`, on workers.dev (`wrangler.jsonc`, no
bindings). `public/_headers` makes `/_astro/*` immutable; HTML keeps the platform's
`max-age=0, must-revalidate`. Never add `Cache-Control` to `/*`: values from two rules are joined.

```sh
npm run deploy:dry   # build, then wrangler deploy --dry-run (no login needed)
npm run deploy       # verify, online link check, wrangler deploy (local wrangler login)
```

CI's `site-deploy` job dry-runs on every PR and, on `main`, runs the online check and deploys
once the repo secret `CLOUDFLARE_API_TOKEN` exists. Until then deploys are manual.
The site URL is baked into `astro.config.mjs` as the default `site` after the first deploy;
`SITE_URL` still overrides it for a build.

`npm run readme:shot` writes `../docs/demo.png`, the README's screenshot of the final frame (dark,
2x), from a built `dist/`.

W8: resolve light CLI accent_2 contrast and update the existing Rust job's checkout@v4 (Node 20
deprecation); the site jobs use v7.
