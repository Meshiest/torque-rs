// Loads a web build of an example in headless Chrome with WebGPU on, waits until the page says its world is drawn, takes a
// screenshot and fails on any console error, page error or failed request, or on a picture of one flat color.
//
// Usage: node check.mjs <url or folder> <screenshot.png> [seconds] [world|menu|cached|refused] [query]
// A folder (a web build, `<target>/web/level-demo`) is served on a free port of 127.0.0.1 for the check; a URL may carry the
// page's query (`?map=office`), and after a folder the query is the fifth argument.
// Needs puppeteer-core in the folder `TORQUE_WEB_TOOLS` names (`npm install --prefix <folder> puppeteer-core`) and Chrome
// (the installed stable channel, or the program `CHROME` names).
//
// The page's root element carries the game's state: `data-torque-frames`, `data-torque-map`, `data-torque-figure`,
// `data-torque-position` (the level demo's `web` module), and `data-torque-refused` (files.js refused a file or the list).
// - `world`: waits for `data-torque-figure="1"`, then for 120 more frames, and takes the screenshot; then it holds W for two
//   seconds, checks the figure walked, and takes a second one (`<screenshot>-walked.png`).
// - `menu`: waits for 120 frames of the menu instead.
// - `cached` (a build that fetches its files): `world`, then the page loaded again in the same browser (`<screenshot>-again`);
//   the first load must request the files `files.json` names and the second none of them (read from the page's cache).
// - `refused`: a fresh browser profile whose every file `files.json` names comes back with one byte changed; the page must
//   show the refusal, start no game and keep nothing in its cache.
import { createRequire } from 'node:module';
import { join, resolve } from 'node:path';
import { writeFile } from 'node:fs/promises';
import { serve } from './serve.mjs';

const MODES = ['world', 'menu', 'cached', 'refused'];
const [where, out, seconds = '90', mode = 'world', query = ''] = process.argv.slice(2);
if (!where || !out || !MODES.includes(mode)) {
  console.error(`usage: node check.mjs <url or folder> <screenshot.png> [seconds] [${MODES.join('|')}] [query]`);
  process.exit(2);
}
const menu = mode === 'menu';
const tools = resolve(process.env.TORQUE_WEB_TOOLS ?? '.');
const puppeteer = createRequire(join(tools, 'package.json'))('puppeteer-core');

const server = /^https?:/.test(where) ? null : await serve(where, 0);
const url = server ? `http://127.0.0.1:${server.address().port}/${query}` : where;
const stem = out.replace(/\.png$/, '');

// The files `files.json` names, by their URL's path (empty for a check that does not need them).
async function listed() {
  if (mode !== 'cached' && mode !== 'refused') return new Map();
  const list = new URL('files.json', url);
  const response = await fetch(list);
  if (!response.ok) throw new Error(`${list}: HTTP ${response.status} (a build that carries its files has no list)`);
  const { files } = await response.json();
  return new Map(files.map((f) => [new URL(f.path, list).pathname, f]));
}
const files = await listed();

const browser = await puppeteer.launch({
  ...(process.env.CHROME ? { executablePath: process.env.CHROME } : { channel: 'chrome' }),
  headless: true,
  args: ['--enable-unsafe-webgpu', '--enable-gpu', '--ignore-gpu-blocklist', '--window-size=1400,900'],
  defaultViewport: { width: 1400, height: 900 },
});
const problems = [];
const log = [];
// Requests for the listed files, since the count was last reset.
let fileRequests = 0;

// A page whose console, errors and requests are watched.
async function watched(context) {
  const page = await context.newPage();
  page.on('console', (m) => {
    const line = `${m.type()}: ${m.text()}`;
    log.push(line);
    if (m.type() === 'error') problems.push(line);
  });
  page.on('pageerror', (e) => problems.push(`page error: ${e.message}`));
  page.on('requestfailed', (r) => problems.push(`request failed: ${r.url()} ${r.failure()?.errorText}`));
  page.on('request', (r) => {
    if (files.has(new URL(r.url()).pathname)) fileRequests++;
  });
  page.on('response', (r) => {
    if (r.status() >= 400 && !r.url().endsWith('/favicon.ico')) problems.push(`HTTP ${r.status()}: ${r.url()}`);
  });
  return page;
}

const state = (page) =>
  page.evaluate(() => {
    const r = document.documentElement;
    return {
      frames: Number(r.dataset.torqueFrames ?? 0),
      map: r.dataset.torqueMap ?? '',
      figure: r.dataset.torqueFigure === '1',
      position: (r.dataset.torquePosition ?? '').split(' ').filter(Boolean).map(Number),
      refused: r.dataset.torqueRefused ?? null,
    };
  });

// Polls the page's state until `done` holds, the deadline passes or a problem shows.
async function wait(page, deadline, done) {
  let s = await state(page);
  while (!done(s) && Date.now() < deadline && problems.length === 0) {
    await new Promise((r) => setTimeout(r, 250));
    s = await state(page);
  }
  return s;
}

// Loads the page, waits for its world (or menu), takes `file`, and walks the figure.
async function play(page, file) {
  const started = Date.now();
  const deadline = started + Number(seconds) * 1000;
  await page.goto(url, { waitUntil: 'load' });
  const adapter = await page.evaluate(async () => {
    const a = await navigator.gpu?.requestAdapter();
    return a ? `${a.info?.vendor ?? '?'} ${a.info?.architecture ?? ''} ${a.info?.description ?? ''}`.trim() : null;
  });
  console.log(`WebGPU adapter: ${adapter}`);
  let s = await wait(page, deadline, (s) => (menu ? s.frames > 0 : s.figure) || s.refused !== null);
  if (s.refused !== null) problems.push(`the page refused its files: ${s.refused}`);
  if (!menu && !s.figure) problems.push(`no figure within ${seconds} s (frames ${s.frames}, map "${s.map}")`);
  const loaded = Date.now();
  const target = s.frames + 120;
  s = await wait(page, deadline + 60_000, (s) => s.frames >= target);
  const ran = (Date.now() - loaded) / 1000;
  console.log(`map "${s.map}", figure ${s.figure}, loaded in ${((loaded - started) / 1000).toFixed(1)} s, ` +
    `${s.frames} frames (${ran > 0 ? ((120 / ran) | 0) : '?'} fps after load)`);
  const canvas = await page.$('canvas');
  const shot = async (file) => {
    const png = canvas ? await canvas.screenshot({ type: 'png' }) : await page.screenshot({ type: 'png' });
    await writeFile(file, png);
    console.log(`screenshot: ${file}`);
    return png;
  };
  const png = await shot(file);
  if (!menu && s.figure && canvas) {
    const from = s.position;
    await canvas.focus();
    await page.keyboard.down('KeyW');
    await new Promise((r) => setTimeout(r, 2000));
    await page.keyboard.up('KeyW');
    const after = await wait(page, Date.now() + 30_000, (t) => t.frames >= s.frames + 150);
    const to = after.position;
    const moved = Math.hypot(to[0] - from[0], to[1] - from[1]);
    console.log(`W held 2 s: the figure walked ${moved.toFixed(2)} from ${from.map((v) => v.toFixed(2))}`);
    if (!(moved > 1)) problems.push(`the figure did not walk (moved ${moved})`);
    await shot(file.replace(/\.png$/, '') + '-walked.png');
  }
  // The picture's colors, counted by the page: a world drawn is far from one flat color.
  const colors = await page.evaluate(async (b64) => {
    const img = new Image();
    img.src = 'data:image/png;base64,' + b64;
    await img.decode();
    const c = document.createElement('canvas');
    c.width = img.width;
    c.height = img.height;
    const ctx = c.getContext('2d');
    ctx.drawImage(img, 0, 0);
    const d = ctx.getImageData(0, 0, c.width, c.height).data;
    const seen = new Set();
    for (let i = 0; i < d.length; i += 4 * 7) seen.add((d[i] >> 3) << 10 | (d[i + 1] >> 3) << 5 | (d[i + 2] >> 3));
    return seen.size;
  }, Buffer.from(png).toString('base64'));
  console.log(`colors in the picture: ${colors}`);
  if (colors < 64) problems.push(`the picture has only ${colors} colors`);
}

// Loads the page with every listed file changed by one byte, and expects the refusal on the page.
async function refused(page) {
  await page.setRequestInterception(true);
  page.on('request', async (r) => {
    if (!files.has(new URL(r.url()).pathname)) return r.continue();
    const bytes = Buffer.from(await (await fetch(r.url())).arrayBuffer());
    bytes[bytes.length >> 1] ^= 0x5a;
    r.respond({ status: 200, contentType: 'application/octet-stream', body: bytes });
  });
  await page.goto(url, { waitUntil: 'load' });
  const s = await wait(page, Date.now() + Number(seconds) * 1000, (s) => s.refused !== null);
  const shown = await page.$eval('#status', (e) => e.textContent).catch(() => '');
  console.log(`the page shows: ${shown}`);
  await writeFile(out, await page.screenshot({ type: 'png' }));
  console.log(`screenshot: ${out}`);
  if (!s.refused?.includes('not the file the list names')) problems.push(`no refusal of a changed file (${s.refused})`);
  if (!shown.includes('refused')) problems.push('the page does not show the refusal');
  if (s.figure || s.frames > 0 || (await page.$('canvas'))) problems.push('the game started on a refused file');
  const kept = await page.evaluate(async () => {
    let n = 0;
    for (const name of await caches.keys()) n += (await (await caches.open(name)).keys()).length;
    return n;
  });
  if (kept !== 0) problems.push(`the cache kept ${kept} files after a refusal`);
}

try {
  if (mode === 'refused') {
    const context = await browser.createBrowserContext();
    await refused(await watched(context));
  } else {
    const page = await watched(browser.defaultBrowserContext());
    await play(page, out);
    if (mode === 'cached') {
      const first = fileRequests;
      fileRequests = 0;
      await play(page, `${stem}-again.png`);
      console.log(`requests for the ${files.size} listed files: ${first} on the first load, ${fileRequests} on the second`);
      if (first < files.size) problems.push(`the first load requested ${first} of the ${files.size} listed files`);
      if (fileRequests !== 0) problems.push(`the second load requested ${fileRequests} listed files (not from the cache)`);
    }
  }
} finally {
  await browser.close();
  server?.close();
}
if (problems.length) {
  console.error('FAILED:\n  ' + problems.join('\n  '));
  console.error('console:\n  ' + log.slice(0, 400).join('\n  '));
  process.exit(1);
}
console.log('ok: no console errors');
