// The arena's player-hosted checks: a room service (`torque-rooms`, serving the browser build) on a free port of
// 127.0.0.1, a host, and headless Chrome joining it by the room's code over a WebRTC data channel.
//
// Usage: node p2p-check.mjs <web build folder> <screenshot.png> [seconds] [desktop|browser|served] [fps|vehicle-demo]
// - `desktop`: the host is `fps-server --room` (a desktop host). The page joins with `?room=<code>`: its figure must show,
//   the datagrams go on the data channel, the figure walks, and no console error shows. Then the host stops, and the page
//   must say that the host left within 15 s.
// - `browser`: the host is a second page (`?host=1`): its server runs in a Web Worker (`data-torque-server="worker"`) and
//   its code shows on the page (`data-torque-room`). The joining page must hold the host's figure and the host page the
//   joiner's, the joiner walks on its data channel, and no console error shows on either. Then the host page closes, and
//   the joiner must say that the host left within 15 s.
// The vehicle demo (`vehicle-demo`, built with `node build.mjs vehicle-demo`) plays its run with two pages (vehicle-mp.mjs):
// in `desktop` both join `vehicle-demo-server --room`, in `browser` the second joins the first, the host page, and in
// `served` (no room service) both join `vehicle-demo-server --transport both --webrtc` at the page it serves (`?join`), on
// data channels. One drives while the other watches, then rides as the passenger, then walks; then the host goes (the
// server stops, in `served` too) and the second page must say so within 15 s.
// Build first: `node build.mjs fps` (the folder is <target>/web/fps). The native programs are built in release by cargo
// here. The browser comes from `guard.mjs` (TORQUE_WEB_TOOLS and CHROME as it reads them), and every page loads with
// `?nolock`: no page can take the pointer lock. The check presses keys only; it never moves or clicks the mouse.
import { spawn, spawnSync } from 'node:child_process';
import { writeFile } from 'node:fs/promises';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchGuarded, noLock } from './guard.mjs';
import { vehicleRun } from './vehicle-mp.mjs';

const [folder, out, seconds = '120', mode = 'desktop', game = 'fps'] = process.argv.slice(2);
// Each game's package and dedicated server, and the host's switches beside `--room`.
const GAMES = {
  fps: { package: 'torque-example-fps', server: 'fps-server', hostArgs: ['--map', 'fixtures/floor.mis'] },
  'vehicle-demo': { package: 'torque-example-vehicle-demo', server: 'vehicle-demo-server', hostArgs: [] },
};
const spec = GAMES[game];
if (!folder || !out || !['desktop', 'browser', 'served'].includes(mode) || !spec || (mode === 'served' && game !== 'vehicle-demo')) {
  console.error('usage: node p2p-check.mjs <web build folder> <screenshot.png> [seconds] [desktop|browser|served] [fps|vehicle-demo]');
  console.error('       (served: vehicle-demo only; for the arena use fps-check.mjs)');
  process.exit(2);
}
const vehicle = game === 'vehicle-demo';
const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '..');
const programs = [
  ...(mode === 'served' ? [] : ['--bin', 'torque-rooms']),
  ...(mode === 'browser' ? [] : ['--bin', spec.server]),
];
const built = spawnSync('cargo', ['build', '--release', '-p', 'torque-rooms', '-p', spec.package, ...programs], {
  cwd: root,
  stdio: 'inherit',
});
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
      if (process.env.FPS_CHECK_VERBOSE) console.log(`${name}: ${line}`);
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

const problems = [];
const log = [];
// Each page in a browser of its own: a page a browser hides (one behind another) stops drawing, and its client with it.
const launch = () =>
  launchGuarded({
    args: [
      '--enable-unsafe-webgpu',
      '--enable-gpu',
      '--ignore-gpu-blocklist',
      '--window-size=1400,900',
      // Two pages on one computer: their host candidates as addresses, not `.local` names.
      '--disable-features=WebRtcHideLocalIpsWithMdns',
    ],
    defaultViewport: { width: 1400, height: 900 },
  });
const browsers = [];


async function watched(name) {
  const browser = await launch();
  browsers.push(browser);
  const page = await browser.newPage();
  page.on('console', (m) => {
    const line = `${name} ${m.type()}: ${m.text()}`;
    log.push(line);
    if (m.type() === 'error') problems.push(line);
  });
  page.on('pageerror', (e) => problems.push(`${name} page error: ${e.message}`));
  page.on('requestfailed', (r) => problems.push(`${name} request failed: ${r.url()} ${r.failure()?.errorText}`));
  page.on('response', (r) => {
    if (r.status() >= 400 && !r.url().endsWith('/favicon.ico')) problems.push(`${name} HTTP ${r.status()}: ${r.url()}`);
  });
  return page;
}

const state = (page) =>
  page.evaluate(() => {
    const r = document.documentElement;
    return {
      frames: Number(r.dataset.torqueFrames ?? 0),
      figure: r.dataset.torqueFigure === '1',
      position: (r.dataset.torquePosition ?? '').split(' ').filter(Boolean).map(Number),
      others: Number(r.dataset.torqueOthers ?? 0),
      link: r.dataset.torqueLink ?? '',
      ended: r.dataset.torqueEnded ?? '',
      status: r.dataset.torqueStatus ?? '',
      room: r.dataset.torqueRoom ?? '',
      server: r.dataset.torqueServer ?? '',
      rtcIn: Number(r.dataset.torqueRtcIn ?? 0),
      seat: Number(r.dataset.torqueSeat ?? -1),
      buggy: (r.dataset.torqueBuggy ?? '').split(' ').filter(Boolean).map(Number),
      buggies: (r.dataset.torqueBuggies ?? '')
        .split(';')
        .filter(Boolean)
        .map((p) => p.split(' ').map(Number)),
    };
  });

async function wait(page, ms, done) {
  const deadline = Date.now() + ms;
  let s = await state(page);
  while (!done(s) && Date.now() < deadline && problems.length === 0) {
    await new Promise((r) => setTimeout(r, 250));
    s = await state(page);
  }
  return s;
}

// The joiner: in the game on the data channel, holding `others` figures, walking.
async function play(page, others) {
  let s = await wait(page, Number(seconds) * 1000, (s) => s.figure || s.ended !== '');
  if (s.ended) problems.push(`the joiner's link ended: ${s.ended}`);
  if (!s.figure) problems.push(`no figure within ${seconds} s (frames ${s.frames}, status "${s.status}")`);
  s = await wait(page, 20_000, (s) => s.link === 'datachannel' && s.others >= others);
  console.log(`joiner: link ${s.link}, ${s.rtcIn} messages on it, other figures ${s.others}`);
  if (s.link !== 'datachannel') problems.push(`the datagrams do not go on the data channel (link ${s.link})`);
  if (s.others < others) problems.push(`the joiner holds ${s.others} other figures, not ${others}`);
  const canvas = await page.$('canvas');
  if (!canvas || problems.length) return;
  await writeFile(out, await canvas.screenshot({ type: 'png' }));
  console.log(`screenshot: ${out}`);
  const from = s.position;
  await canvas.focus();
  const before = s.rtcIn;
  await page.keyboard.down('KeyW');
  await new Promise((r) => setTimeout(r, 2000));
  await page.keyboard.up('KeyW');
  const after = await wait(page, 30_000, (t) => t.frames >= s.frames + 150);
  const moved = Math.hypot(after.position[0] - from[0], after.position[1] - from[1]);
  console.log(`W held 2 s: the figure walked ${moved.toFixed(2)}, ${after.rtcIn - before} messages on the channel`);
  if (!(moved > 1)) problems.push(`the figure did not walk (moved ${moved})`);
  if (!(after.rtcIn > before + 30)) problems.push('the channel stopped bringing messages');
  await writeFile(out.replace(/\.png$/, '') + '-walked.png', await canvas.screenshot({ type: 'png' }));
}

// The host goes (`leave`), and the joiner must say so (`said`: what it says).
async function hostLeaves(page, leave, said = /host left/) {
  await leave();
  const t = Date.now();
  const s = await wait(page, 15_000, (s) => s.ended !== '' && s.status !== '');
  console.log(`after the host left (${((Date.now() - t) / 1000).toFixed(1)} s): "${s.ended}", status "${s.status}"`);
  if (!said.test(s.ended)) problems.push(`the joiner did not say the host left (ended "${s.ended}")`);
  if (!said.test(s.status)) problems.push(`the joiner's menu does not say the host left (status "${s.status}")`);
  await writeFile(out.replace(/\.png$/, '') + '-host-left.png', await page.screenshot({ type: 'png' }));
}

// The vehicle demo's page in the game: its figure, `others` other figures, and when it `joined` a host on a data channel.
async function enter(page, name, others, joined) {
  const s = await wait(
    page,
    Number(seconds) * 1000,
    (s) => (s.figure && s.others >= others && (!joined || s.link === 'datachannel')) || s.ended !== '',
  );
  console.log(`${name}: figure ${s.figure}, link ${s.link}, other figures ${s.others}, seat ${s.seat}`);
  if (s.ended) problems.push(`${name}'s link ended: ${s.ended}`);
  if (!s.figure) problems.push(`${name}: no figure within ${seconds} s (frames ${s.frames}, status "${s.status}")`);
  if (joined && s.link !== 'datachannel') problems.push(`${name}: the datagrams do not go on the data channel (link ${s.link})`);
  if (s.others < others) problems.push(`${name} holds ${s.others} other figures, not ${others}`);
}

// The vehicle demo's two pages at `first` and `second` (URLs), then its run (vehicle-mp.mjs), then the host goes (`leave`).
async function vehicleGame(firstUrl, secondUrl, firstJoined, leave, said) {
  const first = await watched('first');
  await first.goto(noLock(firstUrl), { waitUntil: 'load' });
  await enter(first, 'the first page', 0, firstJoined);
  if (problems.length) return;
  // A host page: its server in its worker, its room's code shown.
  const { room, server } = await wait(first, 20_000, (s) => firstJoined || s.room !== '');
  if (!firstJoined && server !== 'worker') problems.push(`the host's server does not run in a worker ("${server}")`);
  if (!firstJoined && !room) problems.push('the host page shows no room code');
  if (problems.length) return;
  const second = await watched('second');
  await second.goto(noLock(secondUrl(room)), { waitUntil: 'load' });
  await enter(second, 'the second page', 1, true);
  await enter(first, 'the first page', 1, firstJoined);
  if (problems.length) return;
  await writeFile(out, await (await second.$('canvas')).screenshot({ type: 'png' }));
  console.log(`screenshot: ${out}`);
  if (await vehicleRun({ first, second, state, wait, problems, out })) await hostLeaves(second, () => leave(first), said);
}

let status = 1;
try {
  if (vehicle && mode === 'served') {
    const server = start(
      spec.server,
      ['--transport', 'both', '--webrtc', '--port', '0', '--web-root', resolve(folder)],
      'server',
    );
    const url = /browsers join (\S+)/.exec(await server.until(/browsers join https?:\/\//, 60_000))[1];
    console.log(`${spec.server} serves ${url}`);
    const leave = async () => server.child.kill();
    await vehicleGame(`${url}&name=Ann`, () => `${url}&name=Bob`, true, leave, /server closed the connection/);
  } else {
    const rooms = start('torque-rooms', ['--port', '0', '--web-root', resolve(folder)], 'rooms');
    const roomsAt = /rooms at (ws:\/\/\S+)/.exec(await rooms.until(/rooms at ws:\/\//, 30_000))[1];
    const pageAt = /the page at (\S+)/.exec(await rooms.until(/the page at /, 5000))[1];
    console.log(`room service ${roomsAt}, page ${pageAt}`);
    if (mode === 'desktop') {
      const host = start(spec.server, ['--room', roomsAt.replace(/\/ws$/, ''), '--port', '0', ...spec.hostArgs], 'host');
      const code = /room code (\S+)/.exec(await host.until(/room code /, 60_000))[1];
      console.log(`${spec.server} hosts room ${code}`);
      const at = (name) => `${pageAt}?room=${code.toLowerCase()}&name=${name}`;
      if (vehicle) {
        await vehicleGame(at('Ann'), () => at('Bob'), true, async () => host.child.kill(), /host left/);
      } else {
        const page = await watched('joiner');
        await page.goto(noLock(`${pageAt}?room=${code.toLowerCase()}`), { waitUntil: 'load' });
        await play(page, 0);
        if (!problems.length) await hostLeaves(page, async () => host.child.kill());
      }
    } else if (vehicle) {
      const at = (room) => `${pageAt}?room=${room}&name=Bob`;
      await vehicleGame(`${pageAt}?host=1&name=Ann`, at, false, (first) => first.close(), /host left/);
    } else {
      const hostPage = await watched('host');
      await hostPage.goto(noLock(`${pageAt}?host=1&name=Host`), { waitUntil: 'load' });
      let h = await wait(hostPage, Number(seconds) * 1000, (s) => (s.room !== '' && s.figure) || s.ended !== '');
      console.log(`host page: room "${h.room}", server in "${h.server}", figure ${h.figure}`);
      if (h.server !== 'worker') problems.push(`the host's server does not run in a worker ("${h.server}")`);
      if (!h.room) problems.push('the host page shows no room code');
      if (!h.figure) problems.push('the host page has no figure');
      if (!problems.length) {
        const page = await watched('joiner');
        await page.goto(noLock(`${pageAt}?room=${h.room}&name=Joiner`), { waitUntil: 'load' });
        await play(page, 1);
        h = await wait(hostPage, 20_000, (s) => s.others >= 1);
        console.log(`host page: other figures ${h.others}`);
        if (h.others < 1) problems.push('the host page does not hold the joiner\'s figure');
        if (!problems.length) await hostLeaves(page, async () => hostPage.close());
      }
    }
  }
  status = problems.length ? 1 : 0;
} catch (e) {
  problems.push(e.message);
} finally {
  // A failing run leaves the programs a moment to print their status lines (FPS_CHECK_VERBOSE shows them).
  if (problems.length) await new Promise((r) => setTimeout(r, 2000));
  for (const b of browsers) await b.close();
  for (const c of children) c.kill();
}
if (problems.length) {
  console.error('FAILED:\n  ' + problems.join('\n  '));
  console.error('console:\n  ' + log.slice(0, 400).join('\n  '));
  process.exit(1);
}
console.log('ok: no console errors');
process.exit(status);
