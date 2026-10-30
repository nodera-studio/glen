import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
const script=fileURLToPath(new URL('./check-budgets.mjs',import.meta.url));
for (const failure of ['none','missing 404','forbidden bundle','player over budget']) test(`AC-014 budgets: ${failure}`,t=>{
 const cwd=mkdtempSync(join(tmpdir(),'glen-budgets-'));
 t.after(()=>rmSync(cwd,{recursive:true,force:true}));
 mkdirSync(join(cwd,'dist'));
 // A player of random bytes does not compress, so 6 KiB of it overshoots the 5,632 B gzip budget.
 const player=failure==='player over budget' ? `<script>document.querySelector('.term-win');/*${randomBytes(6144).toString('base64')}*/</script>` : '';
 writeFileSync(join(cwd,'dist/index.html'),`<!doctype html><title>glen</title>${player}`);
 if (failure!=='missing 404') writeFileSync(join(cwd,'dist/404.html'),'Not found');
 if (failure==='forbidden bundle') writeFileSync(join(cwd,'dist/planted.js'),"import 'framer-motion';");
 const result=spawnSync(process.execPath,[script],{cwd,encoding:'utf8'});
 assert.equal(result.error,undefined);
 assert.equal(result.status,failure==='none' ? 0 : 1,result.stdout+result.stderr);
 if (failure==='missing 404') assert.match(result.stdout,/404\.html exists.*FAIL/);
 if (failure==='forbidden bundle') assert.match(result.stdout,/forbidden motion bundle: dist\/planted\.js.*FAIL/);
 if (failure==='player over budget') assert.match(result.stdout,/terminal player gzip bytes \| \d+ \| 5632 \| FAIL/);
});
