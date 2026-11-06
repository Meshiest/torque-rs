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
// With a room service the pages switch in place: in `browser` the first page opens as played alone and its menu's Host a
// game hosts there, and in both the second page opens as played alone and joins by the room's code from its menu. Each
// address bar must say what the page plays (`?host=1`, `?room=<code>`). In `browser` the host leaves its game from its
// Esc menu (no page closes): its server, room and code must go. Then the second page (and in `browser` the first) is
// back at its own menu with its address as it opened, offering to host and join by code, and plays alone again.
// No page ever loads again: each page's document is marked once loaded, and the mark must stay throughout.
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
      kept: globalThis.checkMarker === 'kept',
      search: location.search,
    };
  });

// Marks the page's document: the mark stays until the page loads again (state's `kept`).
const markDocument = (page) => page.evaluate(() => (globalThis.checkMarker = 'kept'));

// `name`'s page still holds the document it loaded (`when`: what it did since).
async function sameDocument(page, name, when) {
  const { kept } = await state(page);
  if (!kept) problems.push(`${name} loaded again ${when}`);
}

// Opens `url` in `page` and marks its document.
async function open(page, url) {
  await page.goto(noLock(url), { waitUntil: 'load' });
  await markDocument(page);
}

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

// The host goes (`leave`), and the joiner must say so (`said`: what it says). With `home` the joiner joined in place: its
// link is gone with the game (its marks with it), back at its own menu, which says why.
async function hostLeaves(page, leave, said = /host left/, { home = false } = {}) {
  await leave();
  const t = Date.now();
  const s = await wait(page, 15_000, (s) => (home ? s.link === '' : s.ended !== '') && said.test(s.status));
  console.log(`after the host left (${((Date.now() - t) / 1000).toFixed(1)} s): "${s.ended}", link "${s.link}", status "${s.status}"`);
  if (home && s.link !== '') problems.push(`the joiner's link to the game the host left is still there ("${s.link}")`);
  if (!home && !said.test(s.ended)) problems.push(`the joiner did not say the host left (ended "${s.ended}")`);
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

// The vehicle demo's page `name` back at its own menu, its address `search` (as it opened): it offers to host and join by
// room code, and plays alone again.
async function backHome(page, name, search) {
  const s = await wait(page, 20_000, (s) => !s.figure && s.link === '' && s.room === '' && s.server === '');
  console.log(`${name} at its menu: address "${s.search}", link "${s.link}", room "${s.room}", server "${s.server}"`);
  if (s.figure || s.link || s.room || s.server) {
    problems.push(`${name} is not back at its own menu (link "${s.link}", room "${s.room}", server "${s.server}")`);
  }
  if (s.search !== search) problems.push(`${name}'s address is "${s.search}", not "${search}"`);
  const offered = await page.evaluate(() =>
    globalThis.torque.eval('return HostButton.isVisible() @ JoinAddressButton.isVisible();'),
  );
  console.log(`${name}'s menu offers to host and join by code: ${offered === '11'}`);
  if (offered !== '11') problems.push(`${name}'s menu does not offer to host and join by room code ("${offered}")`);
  if (problems.length) return;
  await page.evaluate(() => globalThis.torque.call('playAlone'));
  const a = await wait(page, 60_000, (s) => s.figure);
  console.log(`${name} plays alone again: figure ${a.figure}, other figures ${a.others}, link "${a.link}"`);
  if (!a.figure) problems.push(`${name} does not play alone again (status "${a.status}")`);
  if (a.others !== 0 || a.link) problems.push(`${name} plays alone with ${a.others} others on link "${a.link}"`);
  await sameDocument(page, name, 'after it went back to its own game');
}

// The vehicle demo's two pages, then its run (vehicle-mp.mjs), then the host goes (`leave`), and the second page must
// say so (`said`). `first`: the first page's URL; with `host` it opens as played alone and its menu hosts. `second`: the
// second page's URL, or with `join` the URL it opens at, played alone, before its menu joins room `join(room)` by code.
// `firstJoined`: the first page joins a game on a data channel (else it hosts). A page that hosted or joined in place goes
// back to the address it opened at once its game is over.
async function vehicleGame({ first: firstUrl, host = false, second: secondUrl, join = null, firstJoined, leave, said }) {
  const first = await watched('first');
  await open(first, firstUrl);
  await enter(first, 'the first page', 0, firstJoined);
  if (problems.length) return;
  const home = (await state(first)).search;
  if (host) {
    // Host a game, from the menu of a page played alone
    await first.evaluate(() => globalThis.torque.call('hostOnPage'));
    const h = await wait(first, 60_000, (s) => (s.room !== '' && s.server === 'worker' && s.link !== '' && s.figure) || s.ended !== '');
    console.log(`the first page hosts in place: room "${h.room}", server "${h.server}", address "${h.search}", figure ${h.figure}`);
    if (!/[?&]host=1(&|$)/.test(h.search)) problems.push(`the host page's address does not say it hosts ("${h.search}")`);
    await sameDocument(first, 'the first page', 'when it hosted a game');
  }
  // A host page: its server in its worker, its room's code shown.
  const { room, server } = await wait(first, 20_000, (s) => firstJoined || s.room !== '');
  if (!firstJoined && server !== 'worker') problems.push(`the host's server does not run in a worker ("${server}")`);
  if (!firstJoined && !room) problems.push('the host page shows no room code');
  if (problems.length) return;
  const second = await watched('second');
  let secondHome = '';
  if (join) {
    await open(second, secondUrl(room));
    await enter(second, 'the second page (played alone)', 0, false);
    if (problems.length) return;
    secondHome = (await state(second)).search;
    // Join room, from its menu: the code as a value, never script text
    const code = join(room);
    await second.evaluate((c) => globalThis.torque.setVar('$World::WebJoinCode', c), code);
  } else {
    await open(second, secondUrl(room));
  }
  await enter(second, 'the second page', 1, true);
  await enter(first, 'the first page', 1, firstJoined);
  if (problems.length) return;
  if (join) {
    const { search } = await state(second);
    console.log(`the second page joined in place: address "${search}"`);
    if (!new URLSearchParams(search).get('room')) problems.push(`the joiner's address does not name its room ("${search}")`);
    await sameDocument(second, 'the second page', 'when it joined a room');
  }
  await writeFile(out, await (await second.$('canvas')).screenshot({ type: 'png' }));
  console.log(`screenshot: ${out}`);
  if (!(await vehicleRun({ first, second, state, wait, problems, out }))) return;
  await hostLeaves(second, () => leave(first), said, { home: Boolean(join) });
  if (problems.length) return;
  await sameDocument(second, 'the second page', 'when the host left');
  if (host) {
    await sameDocument(first, 'the first page', 'when it left its game');
    await backHome(first, 'the first page', home);
  }
  // Back on the menu of a page not played alone: hosting and joining by room code are offered.
  if (join) {
    await backHome(second, 'the second page', secondHome);
  } else {
    const offered = await second.evaluate(() =>
      globalThis.torque.eval('return HostButton.isVisible() @ JoinAddressButton.isVisible();'),
    );
    console.log(`the second page's menu offers to host and join by code: ${offered === '11'}`);
    if (offered !== '11') problems.push(`the menu does not offer to host and join by room code ("${offered}")`);
    await sameDocument(second, 'the second page', 'by the end');
  }
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
    await vehicleGame({
      first: `${url}&name=Ann`,
      second: () => `${url}&name=Bob`,
      firstJoined: true,
      leave,
      said: /server closed the connection/,
    });
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
        await vehicleGame({
          first: at('Ann'),
          second: () => `${pageAt}?name=Bob`,
          join: () => code.toLowerCase(),
          firstJoined: true,
          leave: async () => host.child.kill(),
          said: /host left/,
        });
      } else {
        const page = await watched('joiner');
        await open(page, `${pageAt}?room=${code.toLowerCase()}`);
        await play(page, 0);
        if (!problems.length) await hostLeaves(page, async () => host.child.kill());
        if (!problems.length) await sameDocument(page, 'the joiner', 'when the host left');
      }
    } else if (vehicle) {
      // The host leaves its game from its Esc menu: the page stays open.
      await vehicleGame({
        first: `${pageAt}?name=Ann`,
        host: true,
        second: () => `${pageAt}?name=Bob`,
        join: (room) => room,
        firstJoined: false,
        leave: (first) => first.evaluate(() => globalThis.torque.call('leaveWorld')),
        said: /host left/,
      });
    } else {
      const hostPage = await watched('host');
      await open(hostPage, `${pageAt}?host=1&name=Host`);
      let h = await wait(hostPage, Number(seconds) * 1000, (s) => (s.room !== '' && s.figure) || s.ended !== '');
      console.log(`host page: room "${h.room}", server in "${h.server}", figure ${h.figure}`);
      if (h.server !== 'worker') problems.push(`the host's server does not run in a worker ("${h.server}")`);
      if (!h.room) problems.push('the host page shows no room code');
      if (!h.figure) problems.push('the host page has no figure');
      if (!problems.length) {
        const page = await watched('joiner');
        await open(page, `${pageAt}?room=${h.room}&name=Joiner`);
        await play(page, 1);
        h = await wait(hostPage, 20_000, (s) => s.others >= 1);
        console.log(`host page: other figures ${h.others}`);
        if (h.others < 1) problems.push('the host page does not hold the joiner\'s figure');
        if (!problems.length) await sameDocument(hostPage, 'the host page', 'while the joiner played');
        if (!problems.length) await hostLeaves(page, async () => hostPage.close());
        if (!problems.length) await sameDocument(page, 'the joiner', 'when the host left');
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
