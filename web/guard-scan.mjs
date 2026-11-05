// Fails when a browser check script could load a page without `?nolock` (see `guard.mjs`): every `*check*.mjs` here, and every
// script that names puppeteer, must import `guard.mjs` and must not call `.goto(` itself (`open` does, refusing a URL without
// the option). Usage: node guard-scan.mjs
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const problems = [];
for (const name of readdirSync(here).filter((n) => n.endsWith('.mjs') && n !== 'guard.mjs' && n !== 'guard-scan.mjs')) {
  const text = readFileSync(join(here, name), 'utf8').replace(/^\s*\/\/.*$/gm, '');
  const drives = /check/.test(name) || /puppeteer/.test(text);
  if (!drives) continue;
  if (!/from '\.\/guard\.mjs'/.test(text)) problems.push(`${name}: does not import guard.mjs`);
  if (/\.goto\(/.test(text)) problems.push(`${name}: calls .goto( itself (use open from guard.mjs)`);
}
if (problems.length) {
  console.error(`browser checks must load pages with ?nolock:\n  ${problems.join('\n  ')}`);
  process.exit(1);
}
console.log('every browser check loads its pages with ?nolock');
