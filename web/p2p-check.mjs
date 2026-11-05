// The arena's player-hosted checks: a room service (`torque-rooms`, serving the browser build) on a free port of
// 127.0.0.1, a host, and headless Chrome joining it by the room's code over a WebRTC data channel.
//
// Usage: node p2p-check.mjs <web build folder> <screenshot.png> [seconds] [desktop|browser]
// - `desktop`: the host is `fps-server --room` (a desktop host). The page joins with `?room=<code>`: its figure must show,
//   the datagrams go on the data channel, the figure walks, and no console error shows. Then the host stops, and the page
//   must say that the host left within 15 s.
// - `browser`: the host is a second page (`?host=1`): its server runs in a Web Worker (`data-torque-server="worker"`) and
//   its code shows on the page (`data-torque-room`). The joining page must hold the host's figure and the host page the
//   joiner's, the joiner walks on its data channel, and no console error shows on either. Then the host page closes, and
//   the joiner must say that the host left within 15 s.
// Build first: `node build.mjs fps` (the folder is <target>/web/fps). The native programs are built in release by cargo
// here. TORQUE_WEB_TOOLS and CHROME as `check.mjs` reads them.
import { spawn, spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { writeFile } from 'node:fs/promises';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const [folder, out, seconds = '120', mode = 'desktop'] = process.argv.slice(2);
if (!folder || !out || !['desktop', 'browser'].includes(mode)) {
  console.error('usage: node p2p-check.mjs <web build folder> <screenshot.png> [seconds] [desktop|browser]');
  process.exit(2);
}
const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '..');
const tools = resolve(process.env.TORQUE_WEB_TOOLS ?? '.');
const puppeteer = createRequire(join(tools, 'package.json'))('puppeteer-core');
const programs = ['--bin', 'torque-rooms', ...(mode === 'desktop' ? ['--bin', 'fps-server'] : [])];
const built = spawnSync('cargo', ['build', '--release', '-p', 'torque-rooms', '-p', 'torque-example-fps', ...programs], {
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
const browser = await puppeteer.launch({
  ...(process.env.CHROME ? { executablePath: process.env.CHROME } : { channel: 'chrome' }),
  headless: true,
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

async function watched(name) {
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

// The host goes (`leave`), and the joiner must say so.
async function hostLeaves(page, leave) {
  await leave();
  const t = Date.now();
  const s = await wait(page, 15_000, (s) => s.ended !== '');
  console.log(`after the host left (${((Date.now() - t) / 1000).toFixed(1)} s): "${s.ended}", status "${s.status}"`);
  if (!/host left/.test(s.ended)) problems.push(`the joiner did not say the host left (ended "${s.ended}")`);
  await writeFile(out.replace(/\.png$/, '') + '-host-left.png', await page.screenshot({ type: 'png' }));
}

let status = 1;
try {
  const rooms = start('torque-rooms', ['--port', '0', '--web-root', resolve(folder)], 'rooms');
  const roomsAt = /rooms at (ws:\/\/\S+)/.exec(await rooms.until(/rooms at ws:\/\//, 30_000))[1];
  const pageAt = /the page at (\S+)/.exec(await rooms.until(/the page at /, 5000))[1];
  console.log(`room service ${roomsAt}, page ${pageAt}`);
  if (mode === 'desktop') {
    const host = start('fps-server', ['--room', roomsAt.replace(/\/ws$/, ''), '--port', '0', '--map', 'fixtures/floor.mis'], 'host');
    const code = /room code (\S+)/.exec(await host.until(/room code /, 60_000))[1];
    console.log(`fps-server hosts room ${code}`);
    const page = await watched('joiner');
    await page.goto(`${pageAt}?room=${code.toLowerCase()}`, { waitUntil: 'load' });
    await play(page, 0);
    if (!problems.length) await hostLeaves(page, async () => host.child.kill());
  } else {
    const hostPage = await watched('host');
    await hostPage.goto(`${pageAt}?host=1&name=Host`, { waitUntil: 'load' });
    let h = await wait(hostPage, Number(seconds) * 1000, (s) => (s.room !== '' && s.figure) || s.ended !== '');
    console.log(`host page: room "${h.room}", server in "${h.server}", figure ${h.figure}`);
    if (h.server !== 'worker') problems.push(`the host's server does not run in a worker ("${h.server}")`);
    if (!h.room) problems.push('the host page shows no room code');
    if (!h.figure) problems.push('the host page has no figure');
    if (!problems.length) {
      const page = await watched('joiner');
      await page.goto(`${pageAt}?room=${h.room}&name=Joiner`, { waitUntil: 'load' });
      await play(page, 1);
      h = await wait(hostPage, 20_000, (s) => s.others >= 1);
      console.log(`host page: other figures ${h.others}`);
      if (h.others < 1) problems.push('the host page does not hold the joiner\'s figure');
      if (!problems.length) await hostLeaves(page, async () => hostPage.close());
    }
  }
  status = problems.length ? 1 : 0;
} catch (e) {
  problems.push(e.message);
} finally {
  await browser.close();
  for (const c of children) c.kill();
}
if (problems.length) {
  console.error('FAILED:\n  ' + problems.join('\n  '));
  console.error('console:\n  ' + log.slice(0, 400).join('\n  '));
  process.exit(1);
}
console.log('ok: no console errors');
process.exit(status);
