// Link check for the built site and the root README, run before every deploy.
//   node scripts/check-links.mjs           offline: dist exists; while releaseAvailable is false no
//                                          installer URL or /releases/latest anywhere in dist, and no
//                                          version chip that reads as released; raw and blob/tree URLs
//                                          into this repo's main resolve in the checkout; README
//                                          relative links resolve.
//   node scripts/check-links.mjs --online  the same, then every other absolute URL must answer < 400
//                                          (GET, redirects followed, 3 attempts). CI runs it on main
//                                          pushes; `npm run deploy` runs it before wrangler.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const siteDir = fileURLToPath(new URL('..', import.meta.url));
const TARGETS = ['aarch64-apple-darwin', 'x86_64-apple-darwin', 'x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl'];
const UNRELEASED = [/raw\.githubusercontent\.com\/nodera-studio\/glen/, /\/releases\/latest/];
// What site.ts's versionLabel adds to a version chip before the first release.
const COMING_SOON = /\bcoming soon\b/;
const BINARY = /\.(?:png|jpe?g|gif|webp|avif|ico|woff2?|ttf|otf)$/i;

const entities = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ' };
/** @param {string} s */
const decodeEntities = s => s.replace(/&(?:#(\d+)|#x([0-9a-f]+)|([a-z]+));/gi, (m, dec, hex, name) =>
 dec ? String.fromCodePoint(+dec) : hex ? String.fromCodePoint(parseInt(hex, 16)) : entities[name.toLowerCase()] ?? m);
/** Absolute URLs inside free text, without trailing sentence punctuation. @param {string} text */
const urlsIn = text => [...text.matchAll(/https?:\/\/[^\s"'<>`)\]]+/g)].map(m => m[0].replace(/[.,;:!?]+$/, ''));

/** Absolute http(s) URLs in href/src attributes and in <code> text, entity-decoded, in order, once each. @param {string} html */
export function collectUrls(html) {
 const out = [];
 for (const m of html.matchAll(/\s(?:href|src)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))/gi)) {
  const value = decodeEntities(m[1] ?? m[2] ?? m[3]).trim();
  if (/^https?:\/\//i.test(value)) out.push(value);
 }
 for (const m of html.matchAll(/<code\b[^>]*>([\s\S]*?)<\/code>/gi)) out.push(...urlsIn(decodeEntities(m[1].replace(/<[^>]*>/g, ''))));
 return [...new Set(out)];
}

/** Version chips: the page's text nodes that open with a version tag ("v0.1.0", "v0.1.0 · …"), entity-decoded. @param {string} html */
export const versionChips = html => [...html.replace(/<(script|style)\b[\s\S]*?<\/\1\s*>/gi, '').matchAll(/>([^<]+)</g)]
 .map(m => decodeEntities(m[1]).replace(/\s+/g, ' ').trim()).filter(text => /^v\d+\.\d+\.\d+\b/.test(text));

/** Absolute URLs in Markdown: link targets and bare URLs in code. @param {string} md */
export function markdownUrls(md) {
 const out = [...md.matchAll(/\]\((https?:\/\/[^)\s]+)\)/g)].map(m => m[1]);
 for (const m of md.matchAll(/```[^\n]*\n([\s\S]*?)```|`([^`\n]+)`/g)) out.push(...urlsIn(m[1] ?? m[2]));
 return [...new Set(out)];
}

/** Relative Markdown link and image targets, without their #fragment. @param {string} md */
export const markdownPaths = md => [...new Set([...md.matchAll(/\]\(([^)\s]+)\)/g)].map(m => m[1])
 .filter(t => !/^(?:[a-z]+:|#)/i.test(t)).map(t => t.replace(/[#?].*$/, '')))];

/**
 * @param {string} url @param {string} [siteOrigin]
 * @returns {{kind: 'self'|'ns'|'remote'} | {kind: 'repo-main', path: string}}
 */
export function classify(url, siteOrigin) {
 let parsed;
 try { parsed = new URL(url); } catch { return { kind: 'remote' }; }
 if (siteOrigin && parsed.origin === new URL(siteOrigin).origin) return { kind: 'self' };
 if (url.startsWith('http://www.w3.org/')) return { kind: 'ns' };
 const m = parsed.origin === 'https://raw.githubusercontent.com' ? parsed.pathname.match(/^\/nodera-studio\/glen\/main\/(.+)$/)
  : parsed.origin === 'https://github.com' ? parsed.pathname.match(/^\/nodera-studio\/glen\/(?:blob|tree)\/main(?:\/(.*))?$/)
  : null;
 return m ? { kind: 'repo-main', path: decodeURIComponent(m[1] ?? '').replace(/\/$/, '') } : { kind: 'remote' };
}

/** @param {string} dir @returns {string[]} */
const walk = dir => existsSync(dir) ? readdirSync(dir).flatMap(name => { const p = join(dir, name); return statSync(p).isDirectory() ? walk(p) : [p]; }) : [];

/**
 * The checks that need no network. Returns one row per check and the remote URLs left for --online.
 * @param {{dist: string, root: string, readme?: string, releaseAvailable: boolean, siteOrigin?: string}} o
 */
export function offline({ dist, root, readme, releaseAvailable, siteOrigin }) {
 /** @type {{url: string, status: string, ok: boolean}[]} */
 const rows = [];
 for (const page of ['index.html', '404.html']) rows.push({ url: `dist/${page}`, status: existsSync(join(dist, page)) ? 'exists' : 'missing', ok: existsSync(join(dist, page)) });
 const files = walk(dist).filter(f => !BINARY.test(f));
 /** @type {Map<string, string>} url -> where it was found */
 const urls = new Map();
 for (const file of files) {
  const text = readFileSync(file, 'utf8');
  if (!releaseAvailable) for (const re of UNRELEASED) if (re.test(text)) rows.push({ url: `${relative(dist, file)}: ${re.source.replace(/\\/g, '')}`, status: 'before the first release', ok: false });
  if (!releaseAvailable && file.endsWith('.html')) for (const chip of versionChips(text)) if (!COMING_SOON.test(chip)) rows.push({ url: `${relative(dist, file)}: version chip "${chip}"`, status: 'reads as released before the first release', ok: false });
  if (file.endsWith('.html')) for (const url of collectUrls(text)) if (!urls.has(url)) urls.set(url, relative(dist, file));
 }
 if (readme && existsSync(readme)) {
  const md = readFileSync(readme, 'utf8');
  for (const url of markdownUrls(md)) if (!urls.has(url)) urls.set(url, 'README.md');
  for (const path of markdownPaths(md)) {
   const ok = existsSync(resolve(root, path));
   rows.push({ url: `README.md -> ${path}`, status: ok ? 'in checkout' : 'missing from checkout', ok });
  }
 } else if (readme) console.log(`notice: ${relative(root, readme)} not found; README links not checked`);
 const remote = [];
 for (const url of urls.keys()) {
  const c = classify(url, siteOrigin);
  if (c.kind === 'repo-main') {
   const ok = existsSync(resolve(root, c.path || '.'));
   rows.push({ url, status: ok ? 'in checkout' : 'missing from checkout', ok });
  } else if (c.kind === 'remote') remote.push(url);
 }
 return { rows, remote };
}

/** GET with redirects, 3 attempts (1 s, 2 s backoff); the final status, or 0 on a network error. @param {string} url */
export async function status(url, { attempts = 3, wait = (/** @type {number} */ ms) => new Promise(r => setTimeout(r, ms)) } = {}) {
 let last = 0;
 for (let i = 0; i < attempts; i++) {
  if (i) await wait(1000 * i);
  try {
   const res = await fetch(url, { method: 'GET', redirect: 'follow', headers: { 'user-agent': 'glen-site-link-check' }, signal: AbortSignal.timeout(15000) });
   await res.body?.cancel();
   last = res.status;
   if (last < 400) return last;
  } catch { last = 0; }
 }
 return last;
}

/** The site's own origin: SITE_URL, else the default baked into astro.config.mjs. */
function siteOrigin() {
 if (process.env.SITE_URL) return process.env.SITE_URL;
 const config = readFileSync(join(siteDir, 'astro.config.mjs'), 'utf8');
 return config.match(/process\.env\.SITE_URL\s*\|\|\s*['"](https?:\/\/[^'"]+)['"]/)?.[1];
}

async function main() {
 const online = process.argv.includes('--online');
 const { site } = await import(pathToFileURL(join(siteDir, 'src/data/site.ts')).href);
 const root = resolve(siteDir, '..');
 const { rows, remote } = offline({ dist: join(siteDir, 'dist'), root, readme: join(root, 'README.md'), releaseAvailable: site.releaseAvailable, siteOrigin: siteOrigin() });
 if (online) {
  const extra = site.releaseAvailable ? [site.installScriptUrl, ...TARGETS.flatMap(t => [`${site.latestReleaseUrl}/download/glen-${t}.tar.gz`, `${site.latestReleaseUrl}/download/glen-${t}.tar.gz.sha256`])] : [];
  for (const url of [...new Set([...remote, ...extra])]) {
   const code = await status(url);
   rows.push({ url, status: code ? String(code) : 'network error', ok: code > 0 && code < 400 });
  }
 } else if (remote.length) console.log(`notice: ${remote.length} external URLs not fetched; run with --online to check them`);
 console.log('url | status | ok');
 for (const r of rows) console.log(`${r.url} | ${r.status} | ${r.ok ? 'ok' : 'FAIL'}`);
 const failed = rows.filter(r => !r.ok);
 if (failed.length) console.error(`check-links: ${failed.length} failing ${failed.length === 1 ? 'link' : 'links'}`);
 process.exitCode = failed.length ? 1 : 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) await main();
