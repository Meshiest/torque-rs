// The arena's cross-play check: a dedicated server on UDP and the web (`fps-server --transport both`) on a free port of
// 127.0.0.1, serving the browser build's folder; a desktop player with no window joined over UDP (the `join_bot` example);
// then `check.mjs` in `join` mode: headless Chrome opens the server's URL, joins over WebSocket, must hold the desktop
// player's figure and its own, walk, and show no console error. The servers stop when the check ends.
//
// Usage: node fps-check.mjs <web build folder> <screenshot.png> [seconds] [join|rtc|rtc-loss|rtc-blocked]
// With a data-channel mode the server also takes WebRTC data channels (`--webrtc`); `rtc-loss` drops 10% of their
// datagrams both ways (`--rtc-loss 10`) and reads the server's traffic lines (`--rtc-stats`): while the browser plays,
// no two of its game messages may arrive more than 300 ms apart (no lost packet holds back the ones after it).
// `rtc-blocked` has the server drop every datagram of the channels (UDP blocked on the way): the browser must try the
// channel, give it up and play over its game socket.
// Build first: `node build.mjs fps` (the folder is <target>/web/fps). The native programs are built in release by cargo
// here (CARGO_TARGET_DIR's target folder). TORQUE_WEB_TOOLS and CHROME as `check.mjs` reads them.
import { spawn, spawnSync } from 'node:child_process';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { noLock } from './guard.mjs';

const [folder, out, seconds = '120', mode = 'join'] = process.argv.slice(2);
if (!folder || !out || !['join', 'rtc', 'rtc-loss', 'rtc-blocked'].includes(mode)) {
  console.error('usage: node fps-check.mjs <web build folder> <screenshot.png> [seconds] [join|rtc|rtc-loss|rtc-blocked]');
  process.exit(2);
}
const lossArgs = { 'rtc-loss': ['--rtc-loss', '10', '--rtc-stats'], 'rtc-blocked': ['--rtc-loss', '100'] }[mode] ?? [];
const rtcArgs = mode === 'join' ? [] : ['--webrtc', ...lossArgs];
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
    ['--transport', 'both', '--port', '0', '--map', 'fixtures/floor.mis', '--web-root', resolve(folder), ...rtcArgs],
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
  // The check's lines are passed on, and the moment it holds the other player's figure (the browser in the game) kept.
  const check = spawn(process.execPath, [join(here, 'check.mjs'), noLock(url), out, seconds, mode], {
    stdio: ['ignore', 'pipe', 'inherit'],
  });
  let inGameAt = null;
  check.stdout.on('data', (chunk) => {
    process.stdout.write(chunk);
    if (inGameAt === null && /other players' figures held/.test(chunk.toString())) inGameAt = server.lines.length;
  });
  status = await new Promise((ok) => check.on('exit', (code) => ok(code ?? 1)));
  if (mode === 'rtc-loss') {
    // Each traffic line covers the 2 s before it; the first one after the browser is in the game may hold its joining.
    const windows = server.lines
      .slice(inGameAt ?? server.lines.length)
      .map((l) => /rtc: 2(?:4\d|5[0-5])\.\S+ open, (\d+) messages in, \d+ out, longest gap (\d+) ms/.exec(l))
      .filter(Boolean)
      .map((m) => ({ messages: Number(m[1]), gap: Number(m[2]) }))
      .slice(1);
    console.log(`the server's windows while the browser played: ${JSON.stringify(windows)}`);
    const dropped = server.lines.map((l) => /(\d+) datagrams dropped by the loss test/.exec(l)).filter(Boolean).pop();
    console.log(`datagrams dropped by the loss test: ${dropped?.[1] ?? 0}`);
    if (status === 0 && (windows.length === 0 || windows.some((w) => !(w.gap < 300)))) {
      console.error('FAILED: the browser\'s game messages stalled on the data channel (or no window to judge)');
      status = 1;
    }
    if (status === 0 && !(Number(dropped?.[1] ?? 0) > 0)) {
      console.error('FAILED: the loss test dropped nothing');
      status = 1;
    }
  }
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
