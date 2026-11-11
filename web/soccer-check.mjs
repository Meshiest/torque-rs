// The soccer game's browser checks (the mods demo, examples/mods-demo), as the arena's (fps-check.mjs, p2p-check.mjs): a
// soccer server (`mods-demo-server`, its Lua game mode and its bots) and headless Chrome from the cursor guard joining it.
// The page has none of the server's mods: it must show the download screen with the soccer mod, its files and bytes and a
// bar part way, then play the match (its player runs with D held), then leave and join again from its menu with nothing to
// download (the mod is kept for as long as the page is open). No console error may show.
//
// Usage: node soccer-check.mjs <web build folder> <screenshot.png> [seconds] [join|rtc|room]
// - `join`: the server on UDP and the web (`--transport both`) serves the page; the page joins on its game socket.
// - `rtc`: the same with `--webrtc`: the datagrams (the download among them) must go on the data channel.
// - `room`: a room service (`torque-rooms`, serving the page) and `mods-demo-server --room`; the page joins by the room's
//   code on a data channel.
// Pictures: <screenshot>-download.png (the download screen), <screenshot>.png (the match), <screenshot>-again.png (the
// second join, nothing to download). Build first: `node build.mjs mods-demo`, and the Lua extension (`just lua`). The native
// programs are built in release by cargo here (CARGO_TARGET_DIR's target folder). TORQUE_WEB_TOOLS and CHROME as `check.mjs`
// reads them. The check presses keys only; it never moves or clicks the mouse.
import { spawn, spawnSync } from 'node:child_process';
import { writeFile } from 'node:fs/promises';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchGuarded, noLock } from './guard.mjs';

const [folder, out, seconds = '120', mode = 'join'] = process.argv.slice(2);
if (!folder || !out || !['join', 'rtc', 'room'].includes(mode)) {
  console.error('usage: node soccer-check.mjs <web build folder> <screenshot.png> [seconds] [join|rtc|room]');
  process.exit(2);
}
const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '..');
const bins = ['--bin', 'mods-demo-server', ...(mode === 'room' ? ['--bin', 'torque-rooms'] : [])];
const packages = ['-p', 'torque-example-mods-demo', ...(mode === 'room' ? ['-p', 'torque-rooms'] : [])];
const built = spawnSync('cargo', ['build', '--release', ...packages, ...bins], { cwd: root, stdio: 'inherit' });
if (built.status !== 0) process.exit(built.status ?? 1);
const target = resolve(process.env.CARGO_TARGET_DIR ?? join(root, 'target'), 'release');
// A program's file name ends as Node's own does (nothing outside Windows).
const program = (name) => join(target, name + extname(process.execPath));

const children = [];
function start(file, args, name) {
  const child = spawn(program(file), args, { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] });
  children.push(child);
  const lines = [];
  const waiters = [];
  const take = (chunk) => {
    for (const line of chunk.toString().split(/\r?\n/).filter(Boolean)) {
      lines.push(line);
      if (process.env.SOCCER_CHECK_VERBOSE) console.log(`${name}: ${line}`);
      for (const w of waiters.splice(0)) if (!w(line)) waiters.push(w);
    }
  };
  child.stdout.on('data', take);
  child.stderr.on('data', take);
  const until = (re, ms) =>
    new Promise((ok, fail) => {
      const seen = lines.find((l) => re.test(l));
      if (seen) return ok(seen);
      const timer = setTimeout(() => fail(new Error(`${name}: no line like ${re} within ${ms / 1000} s:\n${lines.join('\n')}`)), ms);
      waiters.push((line) => (re.test(line) ? (clearTimeout(timer), ok(line), true) : false));
    });
  return { child, until, lines };
}

const pause = (ms) => new Promise((r) => setTimeout(r, ms));
const problems = [];
const log = [];

const state = (page) =>
  page.evaluate(() => {
    const r = document.documentElement;
    const [received, total] = (r.dataset.torqueDownload ?? '').split(' ').map(Number);
    return {
      frames: Number(r.dataset.torqueFrames ?? 0),
      join: r.dataset.torqueJoin ?? '',
      received: received || 0,
      total: total || 0,
      mods: r.dataset.torqueMods ?? '',
      team: r.dataset.torqueTeam ?? '',
      figure: r.dataset.torqueFigure === '1',
      position: (r.dataset.torquePosition ?? '').split(' ').filter(Boolean).map(Number),
      attack: Number(r.dataset.torqueAttack ?? 0),
      link: r.dataset.torqueLink ?? '',
      status: r.dataset.torqueStatus ?? '',
    };
  });

// Waits until `test` holds of the page's state, at most `ms`; the last state, and every join state seen.
async function wait(page, ms, test, seen) {
  const end = Date.now() + ms;
  let s = await state(page);
  while (!test(s) && Date.now() < end) {
    seen?.add(s.join);
    await pause(100);
    s = await state(page);
  }
  seen?.add(s.join);
  return s;
}

let status = 1;
let browser;
try {
  let url;
  if (mode === 'room') {
    const rooms = start('torque-rooms', ['--port', '0', '--web-root', resolve(folder)], 'rooms');
    const roomsAt = /rooms at (ws:\/\/\S+)/.exec(await rooms.until(/rooms at ws:\/\//, 30_000))[1];
    const pageAt = /the page at (\S+)/.exec(await rooms.until(/the page at /, 5000))[1];
    const host = start('mods-demo-server', ['--room', roomsAt.replace(/\/ws$/, ''), '--port', '0'], 'host');
    const code = /room code (\S+)/.exec(await host.until(/room code /, 60_000))[1];
    console.log(`room service ${roomsAt}, room ${code}`);
    url = `${pageAt}?room=${code.toLowerCase()}`;
  } else {
    const server = start(
      'mods-demo-server',
      ['--transport', 'both', '--port', '0', '--web-root', resolve(folder), ...(mode === 'rtc' ? ['--webrtc'] : [])],
      'server',
    );
    url = /browsers join (\S+)/.exec(await server.until(/browsers join (https?:\/\/\S+)/, 60_000))[1];
    console.log(`mods-demo-server serves ${url}`);
  }
  browser = await launchGuarded({
    args: ['--enable-unsafe-webgpu', '--enable-gpu', '--ignore-gpu-blocklist', '--window-size=1400,900'],
    defaultViewport: { width: 1280, height: 720 },
  });
  const page = await browser.newPage();
  page.on('console', (m) => {
    const line = `${m.type()}: ${m.text()}`;
    log.push(line);
    if (m.type() === 'error') problems.push(line);
  });
  page.on('pageerror', (e) => problems.push(`page error: ${e.message}`));
  page.on('requestfailed', (r) => problems.push(`request failed: ${r.url()} ${r.failure()?.errorText}`));
  page.on('response', (r) => {
    if (r.status() >= 400 && !r.url().endsWith('/favicon.ico')) problems.push(`HTTP ${r.status()}: ${r.url()}`);
  });
  await page.goto(noLock(url), { waitUntil: 'load', timeout: 60_000 });
  const limit = Number(seconds) * 1000;
  const stem = out.replace(/\.png$/, '');

  // The download screen, part way.
  const seen = new Set();
  let s = await wait(page, limit, (s) => (s.join === 'downloading' && s.total > 0 && s.received / s.total > 0.3) || s.join === 'playing', seen);
  console.log(`download: ${JSON.stringify(s)}`);
  if (s.join !== 'downloading') problems.push(`the page showed no download part way (${[...seen].join(', ')})`);
  else await writeFile(`${stem}-download.png`, await page.screenshot());
  if (s.mods !== 'Soccer 1.0.0') problems.push(`the download screen names ${JSON.stringify(s.mods)}, not Soccer 1.0.0`);

  // The match.
  s = await wait(page, limit, (s) => s.join === 'playing' && s.figure && s.team !== '', seen);
  console.log(`in the match: ${JSON.stringify(s)}; states seen: ${[...seen].join(', ')}`);
  if (s.join !== 'playing' || !s.figure) problems.push(`the page did not play (${JSON.stringify(s)})`);
  if (s.total === 0 || s.received !== s.total) problems.push(`the download did not finish (${s.received} of ${s.total})`);
  if (!seen.has('joining')) problems.push('the screen never said it was joining the match');
  if (mode !== 'join' && s.link !== 'datachannel') problems.push(`the datagrams go on ${s.link || 'nothing'}, not the data channel`);
  // D runs the player to the screen's right: toward the goal its team attacks.
  await (await page.$('canvas'))?.focus();
  const before = (await state(page)).position;
  await page.keyboard.down('d');
  await pause(1000);
  const after = (await state(page)).position;
  await page.keyboard.up('d');
  const ran = before.length === 3 && after.length === 3 ? (after[1] - before[1]) * s.attack : 0;
  console.log(`D held: ${before.join(' ')} to ${after.join(' ')}, attack ${s.attack}`);
  if (!(ran > 1)) problems.push(`the player did not run toward the goal it attacks with D held (${ran.toFixed(2)} along it)`);
  await pause(1500);
  await writeFile(out, await page.screenshot());

  // Out and in again from the menu: nothing to download.
  await page.evaluate(() => globalThis.torque.call('leaveMatch'));
  await pause(1000);
  await page.evaluate(() => globalThis.torque.call('joinPage'));
  const again = new Set();
  s = await wait(page, limit, (s) => s.join === 'cached' || s.join === 'downloading', again);
  if (s.join === 'cached') await writeFile(`${stem}-again.png`, await page.screenshot());
  s = await wait(page, limit, (s) => s.join === 'playing' && s.figure, again);
  console.log(`joined again: ${JSON.stringify(s)}; states seen: ${[...again].join(', ')}`);
  if (!again.has('cached') || again.has('downloading')) problems.push(`the second join downloaded again (${[...again].join(', ')})`);
  if (s.join !== 'playing') problems.push(`the second join did not play (${JSON.stringify(s)})`);
  status = problems.length ? 1 : 0;
} catch (e) {
  problems.push(e.message);
} finally {
  await browser?.close();
  for (const c of children) c.kill();
}
for (const p of problems) console.error(`FAILED: ${p}`);
if (problems.length) console.error(log.slice(-30).join('\n'));
process.exit(problems.length ? 1 : status);
