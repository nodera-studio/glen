import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { checkServedBuild } from './e2e-server-identity.mjs';

const html = '<!doctype html><title>glen</title><script type="module" src="/_astro/a.js"></script>';

/** A dist/ with index.html in a temp dir, removed after the test. */
function dist(t, body = html) {
 const root = mkdtempSync(join(tmpdir(), 'glen-e2e-identity-'));
 t.after(() => rmSync(root, { recursive: true, force: true }));
 mkdirSync(join(root, 'dist'));
 if (body !== null) writeFileSync(join(root, 'dist/index.html'), body);
 return join(root, 'dist');
}

/** A local server on port 0 that answers every request with `body`; closed after the test. */
async function serve(t, body) {
 const server = createServer((_, res) => res.end(body));
 await new Promise(done => server.listen(0, '127.0.0.1', done));
 t.after(() => new Promise(done => server.close(done)));
 return `http://127.0.0.1:${server.address().port}`;
}

test('resolves when the server serves this build byte for byte', async t => {
 await checkServedBuild(await serve(t, html), dist(t));
});

test('rejects a server with a different body, naming the dist path', async t => {
 const d = dist(t), url = await serve(t, 'x');
 await assert.rejects(checkServedBuild(url, d), error => {
  assert.match(error.message, /is not serving this checkout's build/);
  assert.ok(error.message.includes(join(d, 'index.html')), error.message);
  assert.ok(error.message.includes(`${url}/`), error.message);
  return true;
 });
});

test('rejects a missing build with the build hint', async t => {
 const d = dist(t, null);
 await assert.rejects(checkServedBuild(await serve(t, html), d), { message: `e2e: no build at ${d}; run npm run build first` });
});

test('rejects when nothing listens, naming the URL', async t => {
 const server = createServer();
 await new Promise(done => server.listen(0, '127.0.0.1', done));
 const url = `http://127.0.0.1:${server.address().port}`;
 await new Promise(done => server.close(done));
 await assert.rejects(checkServedBuild(url, dist(t)), error => error.message.startsWith(`e2e: GET ${url}/ failed`));
});
