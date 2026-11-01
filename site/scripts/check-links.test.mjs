import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { collectUrls, classify, markdownUrls, markdownPaths, offline, status, versionChips } from './check-links.mjs';
import { site, versionLabel } from '../src/data/site.ts';

test('collectUrls finds href, src and <code> URLs, entity-decoded, and ignores xmlns', () => {
 const html = `<svg xmlns="http://www.w3.org/2000/svg"></svg>
  <a href="https://example.com/a?x=1&amp;y=2">a</a><img src='https://cdn.example.com/i.png'><a href=/local>l</a>
  <code>curl -fsSL https://example.com/install.sh | sh</code><code class="x"><span>https://example.com/b</span>.</code>
  <a href="https://example.com/a?x=1&amp;y=2">dup</a>`;
 assert.deepEqual(collectUrls(html), ['https://example.com/a?x=1&y=2', 'https://cdn.example.com/i.png', 'https://example.com/install.sh', 'https://example.com/b']);
});

test('classify: self, w3.org namespaces, repo main (raw, blob, tree) and remote', () => {
 const site = 'https://glen.example.workers.dev';
 assert.deepEqual(classify('https://glen.example.workers.dev/#install', site), { kind: 'self' });
 assert.deepEqual(classify('http://www.w3.org/2000/svg'), { kind: 'ns' });
 assert.deepEqual(classify('https://raw.githubusercontent.com/nodera-studio/glen/main/install.sh'), { kind: 'repo-main', path: 'install.sh' });
 assert.deepEqual(classify('https://github.com/nodera-studio/glen/blob/main/docs/design.md#a10'), { kind: 'repo-main', path: 'docs/design.md' });
 assert.deepEqual(classify('https://github.com/nodera-studio/glen/tree/main/site/'), { kind: 'repo-main', path: 'site' });
 assert.deepEqual(classify('https://github.com/nodera-studio/glen/tree/main'), { kind: 'repo-main', path: '' });
 for (const url of ['https://github.com/nodera-studio/glen', 'https://github.com/nodera-studio/glen/releases', 'https://raw.githubusercontent.com/nodera-studio/glen/v0.1.0/install.sh', 'https://github.com/other/glen/blob/main/x', 'https://glen.example.workers.dev/'])
  assert.deepEqual(classify(url), { kind: 'remote' }, url);
});

test('Markdown: link targets, bare URLs in code, and relative paths', () => {
 const md = '![demo](docs/demo.png)\nSee [design](docs/design.md#a10-output-contract) and [x](https://example.com/x).\n```sh\ncargo install --git https://example.com/repo --locked\n```\nRun `curl https://example.com/i.sh`. [top](#glen)';
 assert.deepEqual(markdownUrls(md), ['https://example.com/x', 'https://example.com/repo', 'https://example.com/i.sh']);
 assert.deepEqual(markdownPaths(md), ['docs/demo.png', 'docs/design.md']);
});

/** A checkout with dist/ and README.md in a temp dir, removed after the test. */
const roots = [];
function fixture(t, html, readme) {
 const root = mkdtempSync(join(tmpdir(), 'glen-links-'));
 roots.push(root);
 t.after(() => rmSync(root, { recursive: true, force: true }));
 mkdirSync(join(root, 'site/dist/_astro'), { recursive: true });
 writeFileSync(join(root, 'site/dist/index.html'), html);
 writeFileSync(join(root, 'site/dist/404.html'), '<p>404</p>');
 writeFileSync(join(root, 'install.sh'), '#!/bin/sh\n');
 if (readme !== undefined) writeFileSync(join(root, 'README.md'), readme);
 return { root, dist: join(root, 'site/dist'), readme: join(root, 'README.md') };
}
// W12 item 4: every run used to leave its glen-links-* dirs in the temp dir.
after(() => {
 assert.deepEqual(roots.filter(root => existsSync(root)), [], 'fixture dirs left behind');
});
const failures = r => r.rows.filter(row => !row.ok).map(row => row.url);

test('offline: passes a clean dist and leaves remote URLs for --online', t => {
 const f = fixture(t, '<a href="https://github.com/nodera-studio/glen/releases">r</a><code>curl -fsSL https://raw.githubusercontent.com/nodera-studio/glen/main/install.sh | sh</code>', '[d](install.sh)');
 const released = offline({ ...f, releaseAvailable: true });
 assert.deepEqual(failures(released), []);
 assert.deepEqual(released.remote, ['https://github.com/nodera-studio/glen/releases']);
});

test('offline: before the first release, an installer URL or /releases/latest anywhere in dist fails', t => {
 const f = fixture(t, '<a href="https://github.com/nodera-studio/glen/releases/latest">r</a>');
 writeFileSync(join(f.dist, '_astro/Island.js'), 'const u="https://raw.githubusercontent.com/nodera-studio/glen/main/install.sh"');
 const r = offline({ ...f, releaseAvailable: false });
 assert.equal(failures(r).length, 2, failures(r).join('\n'));
 assert.ok(failures(r).some(u => u.startsWith('index.html')) && failures(r).some(u => u.startsWith('_astro/Island.js')));
 assert.deepEqual(failures(offline({ ...f, releaseAvailable: true })), []);
});

test('versionChips: text nodes that open with a version tag, entity-decoded, outside scripts and styles', () => {
 const html = `<span class="version-chip">v0.1.0</span><p class="release-chip"><span aria-hidden="true"></span>v0.1.0 &#183; works
  with omp 18.14.0</p><p>Binaries arrive with the v0.1.0 release.</p><span>Tested with omp 18.14.0</span>
  <script>const v = '<b>v9.9.9</b>'</script><style>.x::after{content:'>v9.9.9<'}</style>`;
 assert.deepEqual(versionChips(html), ['v0.1.0', 'v0.1.0 · works with omp 18.14.0']);
});

// W8-DOC-15: the header, hero and footer chips said v0.1.0 before any release existed.
test('offline: before the first release, a version chip that reads as released fails', t => {
 const released = versionLabel({ ...site, releaseAvailable: true }), unreleased = versionLabel({ ...site, releaseAvailable: false });
 assert.equal(released, `v${site.version}`);
 assert.equal(unreleased, `v${site.version} · coming soon`);
 const chips = label => `<span class="version-chip">${label}</span><p class="release-chip"><span aria-hidden="true"></span>${label} · works with omp 18.14.0</p>`;
 const early = fixture(t, chips(released));
 writeFileSync(join(early.dist, '404.html'), `<footer><span>Tested with omp 18.14.0</span><span>${released}</span></footer>`);
 assert.deepEqual(failures(offline({ ...early, releaseAvailable: false })), [
  `404.html: version chip "${released}"`, `index.html: version chip "${released}"`, `index.html: version chip "${released} · works with omp 18.14.0"`,
 ]);
 assert.deepEqual(failures(offline({ ...early, releaseAvailable: true })), []);
 assert.deepEqual(failures(offline({ ...fixture(t, chips(unreleased)), releaseAvailable: false })), []);
});

test('offline: repo-main URLs and README paths must exist in the checkout; dist pages must exist', t => {
 const f = fixture(t, '<a href="https://github.com/nodera-studio/glen/blob/main/LICENSE">l</a>', '![x](docs/demo.png)\n`https://raw.githubusercontent.com/nodera-studio/glen/main/install.sh`');
 assert.deepEqual(failures(offline({ ...f, releaseAvailable: true })), ['README.md -> docs/demo.png', 'https://github.com/nodera-studio/glen/blob/main/LICENSE']);
 writeFileSync(join(f.root, 'LICENSE'), 'MIT');
 mkdirSync(join(f.root, 'docs')); writeFileSync(join(f.root, 'docs/demo.png'), '');
 assert.deepEqual(failures(offline({ ...f, releaseAvailable: true })), []);
 const empty = fixture(t, '');
 const r = offline({ dist: join(empty.root, 'nope'), root: empty.root, releaseAvailable: false });
 assert.deepEqual(failures(r), ['dist/index.html', 'dist/404.html']);
});

test('status retries, then reports the final code', async () => {
 const real = globalThis.fetch;
 const codes = [503, 502, 200];
 const waits = [];
 globalThis.fetch = async () => ({ status: codes.shift(), body: null });
 try {
  assert.equal(await status('https://example.com', { wait: async ms => { waits.push(ms); } }), 200);
  assert.deepEqual(waits, [1000, 2000]);
  globalThis.fetch = async () => ({ status: 404, body: null });
  assert.equal(await status('https://example.com', { wait: async () => {} }), 404);
  globalThis.fetch = async () => { throw new Error('ENOTFOUND'); };
  assert.equal(await status('https://example.com', { wait: async () => {} }), 0);
 } finally { globalThis.fetch = real; }
});
