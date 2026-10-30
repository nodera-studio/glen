import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { gzipSync } from 'node:zlib';
const walk = dir => existsSync(dir) ? readdirSync(dir).flatMap(name => { const path=join(dir,name); return statSync(path).isDirectory() ? walk(path) : [path]; }) : [];
const bytes = file => readFileSync(file);
const gzip = value => gzipSync(value).length;
const files = walk('dist');
const html = existsSync('dist/index.html') ? readFileSync('dist/index.html','utf8') : '';
const inline = [...html.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script\s*>/gi)].map(m=>m[1]);
const scripts = inline.join('\n');
// The hero terminal player is the inline script that drives .term-win (Terminal.astro).
const player = inline.find(s=>s.includes('.term-win')) ?? '';
let failed = false;
function check(name, measured, budget, ok = measured <= budget) {
 console.log(`${name} | ${measured} | ${budget} | ${ok ? 'ok' : 'FAIL'}`);
 if (!ok) failed = true;
}
console.log('file class | measured | budget | ok');
check('404.html exists', Number(existsSync('dist/404.html')), 1, existsSync('dist/404.html'));
check('index.html exists', Number(Boolean(html)), 1, Boolean(html));
check('JS gzip bytes', files.filter(f=>f.endsWith('.js')).reduce((n,f)=>n+gzip(bytes(f)),0), 110*1024);
check('woff2 bytes', files.filter(f=>f.endsWith('.woff2')).reduce((n,f)=>n+bytes(f).length,0), 100*1024);
check('index.html gzip bytes', gzip(html), 50*1024);
check('external scripts / modulepreload', Number(/<script\b[^>]*\bsrc\s*=|<link\b[^>]*\brel\s*=\s*["']?modulepreload\b/i.test(html)), 0);
check('inline scripts gzip bytes', gzip(scripts), 8*1024);
check('terminal player gzip bytes', gzip(player), 5632);
const forbidden = /(?:\bfrom\s*|\bimport\s*(?:\(\s*)?|\brequire\s*\(\s*)["'](?:framer-motion|usehooks-ts|lucide-react|torph|@radix-ui\/[^"']*|class-variance-authority|next-useragent)(?:\/[^"']*)?["']|<motion\./;
const cliHex = /#(?:505050|B4B5BD|DADADA|45474D|5E6AD2|7170FF|12A594|007F3D|00CA50|A66300|FF9300|DC2626|FF565F|4069FD|94ADFF)\b/i;
for (const file of walk('src').filter(f=>/\.(?:ts|tsx|astro|mjs)$/.test(f))) {
 const content = readFileSync(file,'utf8');
 if (forbidden.test(content)) { check(`forbidden motion source: ${file}`,1,0); }
}
for (const file of files.filter(f=>f.endsWith('.js'))) {
 if (forbidden.test(readFileSync(file,'utf8'))) check(`forbidden motion bundle: ${file}`,1,0);
}
for (const file of walk('src/components/terminal')) {
 if (cliHex.test(readFileSync(file,'utf8'))) check(`literal CLI token: ${relative('.',file)}`,1,0);
}
if (!failed) console.log('source guards | 0 violations | 0 | ok');
process.exitCode = failed ? 1 : 0;
