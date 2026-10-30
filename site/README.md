# glen site

Static Astro 7, React 19 islands and Tailwind v4. Use Node from `.nvmrc` (24.22.1).

```sh
npm ci
npm run dev
npm run verify       # check, static build, player and budget unit tests, budgets
npx playwright install chromium  # once
npm run test:e2e     # Chromium smoke; preview starts automatically
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
`src/data/demo.mjs` is synthetic but laid out with ports of glen's own table, wrap and
transcript code: W7/W8 must refresh it from real glen output against `tests/fixtures/`.
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

W7: publish `install.sh` at the GitHub raw URL in `src/data/site.ts` and v0.1.0 assets,
then flip `releaseAvailable` and verify Script help against the actual installer.
Add `wrangler.jsonc` with `assets.directory: "./dist"` and `not_found_handling: "404-page"`.
Set `SITE_URL` for canonical/OG URLs and check install/release URLs before deployment.
W7/W8: refresh the synthetic demo. W8: resolve light CLI accent_2 contrast and update
the existing Rust job's checkout@v4 (Node 20 deprecation); the new site job uses v7.
