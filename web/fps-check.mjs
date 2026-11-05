// The arena's cross-play check: a dedicated server on UDP and the web (`fps-server --transport both`) on a free port of
// 127.0.0.1, serving the browser build's folder; a desktop player with no window joined over UDP (the `join_bot` example);
// then `check.mjs` in `join` mode: headless Chrome opens the server's URL, joins over WebSocket, must hold the desktop
// player's figure and its own, walk, and show no console error. The servers stop when the check ends.
//
// Usage: node fps-check.mjs <web build folder> <screenshot.png> [seconds]
// Build first: `node build.mjs fps` (the folder is <target>/web/fps). The native programs are built in release by cargo
// here (CARGO_TARGET_DIR's target folder). TORQUE_WEB_TOOLS and CHROME as `check.mjs` reads them.
import { spawn, spawnSync } from 'node:child_process';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const [folder, out, seconds = '120'] = process.argv.slice(2);
if (!folder || !out) {
  console.error('usage: node fps-check.mjs <web build folder> <screenshot.png> [seconds]');
  process.exit(2);
}
const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '..');
const cargo = ['--release', '-p', 'torque-example-fps'];
const built = spawnSync('cargo', ['build', ...cargo, '--bin', 'fps-server', '--example', 'join_bot'], {
  cwd: root,
  stdio: 'inherit',
});
if (built.status !== 0) process.exit(built.status ?? 1);

const target = resolve(process.env.CARGO_TARGET_DIR ?? join(root, 'target'), 'release');
// A program's file name ends as Node's own does (nothing outside Windows).
const program = (path) => join(target, path + extname(process.execPath));
const children = [];
function start(file, args, name) {
  const child = spawn(program(file), args, { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] });
  children.push(child);
  const lines = [];
  const waiters = [];
  const take = (chunk) => {
    for (const line of chunk.toString().split(/\r?\n/).filter(Boolean)) {
      lines.push(line);
      if (process.env.FPS_CHECK_VERBOSE) console.log(`${name}: ${line}`);
      for (const w of waiters.splice(0)) if (!w(line)) waiters.push(w);
    }
  };
  child.stdout.on('data', take);
  child.stderr.on('data', take);
  // Resolves with the first line `test` matches (a line already printed included).
  const until = (test, ms) =>
    new Promise((ok, fail) => {
      const seen = lines.find(test);
      if (seen) return ok(seen);
      const timer = setTimeout(() => fail(new Error(`${name}: no line like ${test} within ${ms / 1000} s:\n${lines.join('\n')}`)), ms);
      waiters.push((line) => (test(line) ? (clearTimeout(timer), ok(line), true) : false));
    });
  return { child, until: (re, ms) => until((l) => re.test(l), ms), lines };
}

let status = 1;
try {
  const server = start(
    'fps-server',
    ['--transport', 'both', '--port', '0', '--map', 'fixtures/floor.mis', '--web-root', resolve(folder)],
    'server',
  );
  const ready = await server.until(/browsers join (https?:\/\/\S+)/, 60_000);
  const url = /browsers join (\S+)/.exec(ready)[1];
  const at = /on (\d+\.\d+\.\d+\.\d+:\d+)/.exec(await server.until(/ on \d+\.\d+\.\d+\.\d+:\d+/, 1000))[1];
  console.log(`fps-server at ${at}, page ${url}`);
  const bot = start(join('examples', 'join_bot'), [at, '--seconds', String(Number(seconds) + 60)], 'bot');
  await bot.until(/join-bot: in game/, 60_000);
  console.log('the desktop player is in the game');
  // Not spawnSync: the server's and the player's output must go on being read meanwhile.
  const check = spawn(process.execPath, [join(here, 'check.mjs'), url, out, seconds, 'join'], { stdio: 'inherit' });
  status = await new Promise((ok) => check.on('exit', (code) => ok(code ?? 1)));
  const both = bot.lines.some((l) => /join-bot: 2 figures/.test(l));
  console.log(`the desktop player held the browser player's figure: ${both}`);
  if (status === 0 && !both) {
    console.error('FAILED: the desktop player did not hold the browser player\'s figure');
    status = 1;
  }
} catch (e) {
  console.error(`FAILED: ${e.message}`);
  status = 1;
} finally {
  for (const c of children) c.kill();
}
process.exit(status);
