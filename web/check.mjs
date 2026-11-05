// Loads a web build of an example in headless Chrome with WebGPU on, waits until the page says its world is drawn, takes a
// screenshot and fails on any console error, page error or failed request, or on a picture of one flat color.
//
// Usage: node check.mjs <url or folder> <screenshot.png> [seconds] [world|menu] [query]
// A folder (a web build, `<target>/web/level-demo`) is served on a free port of 127.0.0.1 for the check; a URL may carry the
// page's query (`?map=office`), and after a folder the query is the fifth argument.
// Needs puppeteer-core in the folder `TORQUE_WEB_TOOLS` names (`npm install --prefix <folder> puppeteer-core`) and Chrome
// (the installed stable channel, or the program `CHROME` names).
//
// The page's root element carries the game's state: `data-torque-frames`, `data-torque-map`, `data-torque-figure`,
// `data-torque-position` (the level demo's `web` module). The check waits for `data-torque-figure="1"`, then for 120 more
// frames, and takes the screenshot; then it holds W for two seconds, checks the figure walked, and takes a second one
// (`<screenshot>-walked.png`). With `menu` it waits for 120 frames of the menu instead.
import { createRequire } from 'node:module';
import { join, resolve } from 'node:path';
import { writeFile } from 'node:fs/promises';
import { serve } from './serve.mjs';

const [where, out, seconds = '90', mode = 'world', query = ''] = process.argv.slice(2);
const menu = mode === 'menu';
if (!where || !out) {
  console.error('usage: node check.mjs <url or folder> <screenshot.png> [seconds] [world|menu] [query]');
  process.exit(2);
}
const tools = resolve(process.env.TORQUE_WEB_TOOLS ?? '.');
const puppeteer = createRequire(join(tools, 'package.json'))('puppeteer-core');

const server = /^https?:/.test(where) ? null : await serve(where, 0);
const url = server ? `http://127.0.0.1:${server.address().port}/${query}` : where;

const browser = await puppeteer.launch({
  ...(process.env.CHROME ? { executablePath: process.env.CHROME } : { channel: 'chrome' }),
  headless: true,
  args: ['--enable-unsafe-webgpu', '--enable-gpu', '--ignore-gpu-blocklist', '--window-size=1400,900'],
  defaultViewport: { width: 1400, height: 900 },
});
const problems = [];
const log = [];
try {
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
  const started = Date.now();
  await page.goto(url, { waitUntil: 'load' });
  const adapter = await page.evaluate(async () => {
    const a = await navigator.gpu?.requestAdapter();
    return a ? `${a.info?.vendor ?? '?'} ${a.info?.architecture ?? ''} ${a.info?.description ?? ''}`.trim() : null;
  });
  console.log(`WebGPU adapter: ${adapter}`);
  const state = () =>
    page.evaluate(() => {
      const r = document.documentElement;
      return {
        frames: Number(r.dataset.torqueFrames ?? 0),
        map: r.dataset.torqueMap ?? '',
        figure: r.dataset.torqueFigure === '1',
        position: (r.dataset.torquePosition ?? '').split(' ').filter(Boolean).map(Number),
      };
    });
  const wait = async (done) => {
    let s = await state();
    while (!done(s) && Date.now() < deadline && problems.length === 0) {
      await new Promise((r) => setTimeout(r, 250));
      s = await state();
    }
    return s;
  };
  const deadline = started + Number(seconds) * 1000;
  let s = await wait((s) => (menu ? s.frames > 0 : s.figure));
  if (!menu && !s.figure) problems.push(`no figure within ${seconds} s (frames ${s.frames}, map "${s.map}")`);
  const loaded = Date.now();
  const target = s.frames + 120;
  s = await wait((s) => s.frames >= target);
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
  const png = await shot(out);
  if (!menu && s.figure && canvas) {
    const from = s.position;
    await canvas.focus();
    await page.keyboard.down('KeyW');
    await new Promise((r) => setTimeout(r, 2000));
    await page.keyboard.up('KeyW');
    const after = await wait((t) => t.frames >= s.frames + 150);
    const to = after.position;
    const moved = Math.hypot(to[0] - from[0], to[1] - from[1]);
    console.log(`W held 2 s: the figure walked ${moved.toFixed(2)} from ${from.map((v) => v.toFixed(2))}`);
    if (!(moved > 1)) problems.push(`the figure did not walk (moved ${moved})`);
    await shot(out.replace(/\.png$/, '') + '-walked.png');
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
