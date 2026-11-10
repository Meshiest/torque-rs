// The client-script host's cases in a headless browser: the folder `node web/lua-build.mjs --client --check` makes is
// served on a free port of 127.0.0.1 and its page (client-script-check.html) runs the cases (web/client-script-cases.js),
// each realm's guest in a Worker. Every case must pass, with no page error. Under Node the same cases run from the folder
// itself (client-script-node.mjs).
//
// Usage: node client-script-check.mjs [folder]
// The folder defaults to <target>/lua-web/client-check. The browser comes from guard.mjs (TORQUE_WEB_TOOLS, default
// <target>/web-tools/node, and CHROME as it reads them); the page loads with `?nolock`, and no case touches the canvas or
// the pointer.
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchGuarded, noLock } from './guard.mjs';
import { report } from './lua-report.js';
import { serve } from './serve.mjs';

const args = process.argv.slice(2);
if (args.length > 1 || args.some((a) => a.startsWith('--'))) {
  console.error('usage: node client-script-check.mjs [folder]');
  process.exit(2);
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const target = resolve(process.env.CARGO_TARGET_DIR ?? join(root, 'target'));
const folder = resolve(args[0] ?? join(target, 'lua-web', 'client-check'));
process.env.TORQUE_WEB_TOOLS ??= join(target, 'web-tools', 'node');

const server = await serve(folder, 0);
const browser = await launchGuarded();
let ok = false;
try {
  const page = await browser.newPage();
  const problems = [];
  page.on('pageerror', (error) => problems.push(`page error: ${error.message}`));
  await page.goto(noLock(`http://127.0.0.1:${server.address().port}/client-script-check.html`));
  await page.waitForFunction(() => window.clientScriptResults !== undefined, { timeout: 600_000, polling: 500 });
  const results = await page.evaluate(() => window.clientScriptResults);
  ok = report(`${await browser.version()}`, results);
  for (const problem of problems) console.log(problem);
  ok = ok && problems.length === 0;
} finally {
  await browser.close();
  server.close();
}
process.exit(ok ? 0 : 1);
