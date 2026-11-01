import { existsSync, readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
// Playwright globalSetup: refuse to test a server that is not serving this checkout's build
// (a stale `astro preview` from another checkout once answered on :4321). A fresh
// `astro preview` serves dist/index.html byte for byte, so the bodies are compared whole.
export async function checkServedBuild(baseURL, distDir) {
 const dist = resolve(distDir), index = join(dist, 'index.html');
 if (!existsSync(index)) throw new Error(`e2e: no build at ${dist}; run npm run build first`);
 const url = new URL('/', baseURL).href;
 let body;
 try { body = Buffer.from(await (await fetch(url, { signal: AbortSignal.timeout(10_000) })).arrayBuffer()); }
 catch (error) { throw new Error(`e2e: GET ${url} failed: ${error.cause?.message ?? error.message}`); }
 if (!body.equals(readFileSync(index)))
  throw new Error(`e2e: the server at ${url} is not serving this checkout's build (${index} differs). Stop it (lsof -nP -iTCP:${new URL(url).port || 80} -sTCP:LISTEN) or rebuild; GLEN_E2E_REUSE=1 reuses a server only when it serves this build.`);
}
export default async function globalSetup() {
 await checkServedBuild('http://127.0.0.1:4321', fileURLToPath(new URL('../dist', import.meta.url)));
}
