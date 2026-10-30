// Screenshot the hero terminal's final frame, dark theme, into ../docs/demo.png for the README.
// Needs a built dist/ (npm run build). Usage: npm run readme:shot
import { spawn } from 'node:child_process';
import { existsSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';

const siteDir = fileURLToPath(new URL('..', import.meta.url));
const out = join(siteDir, '..', 'docs', 'demo.png');
const port = 4329;
const base = `http://127.0.0.1:${port}`;
const MAX_BYTES = 500 * 1024;

if (!existsSync(join(siteDir, 'dist', 'index.html'))) {
 console.error('readme-shot: dist/index.html is missing; run npm run build first');
 process.exit(1);
}
// astro's own bin rather than npx, so killing the child stops the server itself. --ignore-lock
// keeps it in the foreground: under a coding agent astro would otherwise background the server.
const server = spawn(process.execPath, [join(siteDir, 'node_modules/astro/bin/astro.mjs'), 'preview', '--host', '127.0.0.1', '--port', String(port), '--ignore-lock'], { cwd: siteDir, stdio: 'ignore' });
const browser = await (async () => {
 const deadline = Date.now() + 30_000;
 for (;;) {
  try { if ((await fetch(base + '/')).status === 200) break; } catch {}
  if (Date.now() > deadline) throw new Error(`astro preview did not answer on ${base} within 30 s`);
  await new Promise(r => setTimeout(r, 250));
 }
 return chromium.launch();
})().catch(e => { server.kill(); throw e; });
try {
 const page = await browser.newPage({ viewport: { width: 1200, height: 820 }, deviceScaleFactor: 2, colorScheme: 'dark' });
 await page.goto(base + '/?term=end&theme=dark');
 await page.locator('#term[data-term=done]').waitFor();
 await page.evaluate(() => document.fonts.ready);
 await page.locator('#term .term-win').screenshot({ path: out, animations: 'disabled' });
} finally {
 await browser.close();
 server.kill();
}
const bytes = statSync(out).size;
console.log(`readme-shot: wrote ${out} (${Math.round(bytes / 1024)} KB)`);
if (bytes > MAX_BYTES) { console.error(`readme-shot: ${bytes} bytes is over the ${MAX_BYTES}-byte budget`); process.exitCode = 1; }
