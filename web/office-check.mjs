// The level demo's office joined from a browser (examples/level-demo), as the soccer checks join theirs (soccer-check.mjs): an
// office server (`office-server`, the catalog mod required of its players) and headless Chrome from the cursor guard joining it
// with the page built with the server-scripts runner (`node web/build.mjs level-demo --client-scripts`). The page has none of
// the server's mods: it must show the join screen with the catalog coming in (a picture of it part way when a look catches it:
// the download can be over within a frame or two), enter the office, show the consent prompt, run the catalog in its Worker
// once Allow is pressed, and put a chair down from the catalog's Chair card. No console error may show.
//
// Usage: node office-check.mjs <web build folder> <screenshot.png> [seconds] [join|rtc|room|wt]
// - `join`: the server on UDP and the web (`--transport both`) serves the page; the page joins on its game socket.
// - `rtc`: the same with `--webrtc`: the datagrams (the download among them) must go on the data channel.
// - `room`: a room service (`torque-rooms`, serving the page) and `office-server --room`; the page joins by the room's code
//   on a data channel.
// - `wt`: the server on the web with the QUIC link beside it (`--transport web --webtransport`): the page (on the loopback
//   address, a secure context) opens a WebTransport session with the server's certificate hashes and the datagrams must go
//   on it; no game socket is opened.
// Pictures: <screenshot>-download.png (the join screen), <screenshot>-consent.png (the prompt), <screenshot>-catalog.png (the
// catalog open in the office), <screenshot>.png (the office a few seconds after the chair was put down). The native programs are built in release by cargo here (CARGO_TARGET_DIR's target folder).
// TORQUE_WEB_TOOLS and CHROME as `check.mjs` reads them.
//
// The check never moves the mouse or takes the pointer: the page loads with `?nolock`, and the two clicks (Allow, the Chair
// card) are pointer events the page dispatches on its canvas, where the game marks those buttons (`data-torque-allow`,
// `data-torque-chair`). A page's own events are never a user's gesture, so they cannot lock the pointer either.
import { spawn, spawnSync } from 'node:child_process';
import { writeFile } from 'node:fs/promises';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchGuarded, noLock } from './guard.mjs';

const [folder, out, seconds = '120', mode = 'join'] = process.argv.slice(2);
if (!folder || !out || !['join', 'rtc', 'room', 'wt'].includes(mode)) {
  console.error('usage: node office-check.mjs <web build folder> <screenshot.png> [seconds] [join|rtc|room|wt]');
  process.exit(2);
}
const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '..');
const bins = ['--bin', 'office-server', ...(mode === 'room' ? ['--bin', 'torque-rooms'] : [])];
const packages = ['-p', 'torque-example-level-demo', ...(mode === 'room' ? ['-p', 'torque-rooms'] : [])];
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
      if (process.env.OFFICE_CHECK_VERBOSE) console.log(`${name}: ${line}`);
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
      joins: (r.dataset.torqueJoins ?? '').split(' ').filter(Boolean),
      received: received || 0,
      total: total || 0,
      mods: r.dataset.torqueMods ?? '',
      realm: r.dataset.torqueRealm ?? '',
      allow: r.dataset.torqueAllow ?? '',
      cards: Number(r.dataset.torqueCards ?? 0),
      chair: r.dataset.torqueChair ?? '',
      catalog: r.dataset.torqueCatalogStatus ?? '',
      recent: r.dataset.torqueCatalogRecent ?? '',
      props: Number(r.dataset.torqueProps ?? 0),
      figure: r.dataset.torqueFigure === '1',
      link: r.dataset.torqueLink ?? '',
      wt: r.dataset.torqueWt ?? '',
      rtcIn: Number(r.dataset.torqueRtcIn ?? 0),
      status: r.dataset.torqueStatus ?? '',
      error: r.dataset.torqueError ?? '',
    };
  });

// Waits until `test` holds of the page's state, at most `ms`, looking every `every` ms; the last state, and every join state
// seen.
async function wait(page, ms, test, seen, every = 100) {
  const end = Date.now() + ms;
  let s = await state(page);
  while (!test(s) && Date.now() < end) {
    seen?.add(s.join);
    await pause(every);
    s = await state(page);
  }
  seen?.add(s.join);
  return s;
}

// A click the page dispatches on its canvas at `shares` ("x y", shares of the canvas's width and height): a move there, the
// left button down and up.
async function clickAt(page, shares) {
  const [fx, fy] = shares.split(' ').map(Number);
  await page.evaluate(
    ([fx, fy]) => {
      const canvas = document.querySelector('canvas');
      const r = canvas.getBoundingClientRect();
      const at = { clientX: r.left + fx * r.width, clientY: r.top + fy * r.height };
      const base = { ...at, bubbles: true, cancelable: true, composed: true, pointerId: 1, pointerType: 'mouse', isPrimary: true };
      canvas.dispatchEvent(new PointerEvent('pointermove', { ...base, button: -1, buttons: 0 }));
    },
    [fx, fy],
  );
  // A frame or two with the pointer over the control before the press, as a player's hand gives it.
  await pause(200);
  await page.evaluate(
    ([fx, fy]) => {
      const canvas = document.querySelector('canvas');
      const r = canvas.getBoundingClientRect();
      const at = { clientX: r.left + fx * r.width, clientY: r.top + fy * r.height };
      const base = { ...at, bubbles: true, cancelable: true, composed: true, pointerId: 1, pointerType: 'mouse', isPrimary: true };
      canvas.dispatchEvent(new PointerEvent('pointerdown', { ...base, button: 0, buttons: 1 }));
      canvas.dispatchEvent(new PointerEvent('pointerup', { ...base, button: 0, buttons: 0 }));
    },
    [fx, fy],
  );
}

let browser;
try {
  let url;
  if (mode === 'room') {
    const rooms = start('torque-rooms', ['--port', '0', '--web-root', resolve(folder)], 'rooms');
    const roomsAt = /rooms at (ws:\/\/\S+)/.exec(await rooms.until(/rooms at ws:\/\//, 30_000))[1];
    const pageAt = /the page at (\S+)/.exec(await rooms.until(/the page at /, 5000))[1];
    const host = start('office-server', ['--room', roomsAt.replace(/\/ws$/, ''), '--port', '0'], 'host');
    const code = /room code (\S+)/.exec(await host.until(/room code /, 60_000))[1];
    console.log(`room service ${roomsAt}, room ${code}`);
    url = `${pageAt}?room=${code.toLowerCase()}`;
  } else {
    const transport = mode === 'wt' ? ['--transport', 'web', '--webtransport', '--identity', join(resolve(folder), '..', 'office-check-identity.key')] : ['--transport', 'both'];
    const server = start(
      'office-server',
      [...transport, '--port', '0', '--web-root', resolve(folder), ...(mode === 'rtc' ? ['--webrtc'] : [])],
      'server',
    );
    url = /browsers join (\S+)/.exec(await server.until(/browsers join (https?:\/\/\S+)/, 60_000))[1];
    // The page must be a secure context for WebTransport: plain http on the loopback address is one. (Not `localhost`: the
    // browser may resolve it to the IPv6 loopback, where no server listens.)
    console.log(`office-server serves ${url}`);
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

  // The join screen, the catalog part way down when a look catches it: on the loopback address the mod can come in between two
  // of the page's frames. The page keeps every state the join went through.
  const seen = new Set();
  let s = await wait(page, limit, (s) => (s.join === 'downloading' && s.total > 0 && s.received / s.total > 0.3) || s.join === 'playing', seen, 5);
  console.log(`download: ${JSON.stringify(s)}`);
  if (s.join === 'downloading') await writeFile(`${stem}-download.png`, await page.screenshot());
  else console.log('the download was over before a look caught it part way: no picture of it');
  if (s.mods !== 'Prop catalog 1.0.0') problems.push(`the join screen names ${JSON.stringify(s.mods)}, not Prop catalog 1.0.0`);

  // The office, and the question about the server's scripts.
  s = await wait(page, limit, (s) => s.join === 'playing' && s.figure && s.realm === 'asking' && s.allow !== '', seen);
  console.log(`asked: ${JSON.stringify(s)}; states seen: ${[...seen].join(', ')}`);
  if (s.join !== 'playing' || !s.figure) problems.push(`the page did not enter the office (${JSON.stringify(s)})`);
  if (s.total === 0 || s.received !== s.total) problems.push(`the download did not finish (${s.received} of ${s.total})`);
  if (!s.joins.includes('downloading')) problems.push(`the join never showed the download (${s.joins.join(', ')})`);
  if (s.realm !== 'asking') throw new Error(`the player was not asked about the server's scripts (${JSON.stringify(s)})`);
  if (mode === 'rtc' || mode === 'room') {
    if (s.link !== 'datachannel') problems.push(`the datagrams go on ${s.link || 'nothing'}, not the data channel`);
  } else if (mode === 'wt') {
    if (s.wt !== 'open') problems.push(`the WebTransport session did not open (${s.wt || 'nothing said'})`);
    if (s.link !== 'webtransport') problems.push(`the datagrams go on ${s.link || 'nothing'}, not the WebTransport session`);
    if (s.rtcIn === 0) problems.push('no datagram came over the WebTransport session');
    console.log(`WebTransport carried ${s.rtcIn} datagrams so far`);
  }
  // The prompt ignores the pointer for its first half second.
  await pause(1500);
  await writeFile(`${stem}-consent.png`, await page.screenshot());
  await clickAt(page, (await state(page)).allow);

  // The catalog, run in the page's Worker, and a chair from it.
  s = await wait(page, limit, (s) => s.realm === 'running' && s.cards > 0 && s.chair !== '');
  console.log(`the catalog: ${JSON.stringify(s)}`);
  if (s.realm !== 'running') throw new Error(`the catalog did not start after Allow (${JSON.stringify(s)})`);
  if (s.cards < 20) problems.push(`the catalog shows ${s.cards} cards`);
  await pause(500);
  await writeFile(`${stem}-catalog.png`, await page.screenshot());
  const before = s.props;
  await clickAt(page, s.chair);
  s = await wait(page, limit / 4, (s) => s.props > before && s.recent.startsWith('Browser player put down a chair'));
  console.log(`after the Chair card: ${JSON.stringify(s)}`);
  if (!(s.props > before)) problems.push(`no new prop came (${before} before, ${s.props} after)`);
  if (!s.recent.startsWith('Browser player put down a chair')) {
    problems.push(`the list of spawns says ${JSON.stringify(s.recent)}, the status line ${JSON.stringify(s.catalog)}`);
  }
  // Still in the office a few seconds on: the new prop's updates read like any other's.
  await pause(3000);
  s = await state(page);
  if (s.join !== 'playing' || !s.figure) problems.push(`the page left the office after the chair: ${JSON.stringify(s)}`);
  await writeFile(out, await page.screenshot());
} catch (e) {
  problems.push(e.message);
} finally {
  await browser?.close();
  for (const c of children) c.kill();
}
for (const p of problems) console.error(`FAILED: ${p}`);
if (problems.length) console.error(log.slice(-30).join('\n'));
process.exit(problems.length ? 1 : 0);
