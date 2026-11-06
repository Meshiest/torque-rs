// Checks the website `site.mjs` built, served on 127.0.0.1 under a project site's path (`/torque-rs/`), in headless Chrome with
// WebGPU on: the gallery loads with no console error and no request off the server, its page is no wider than a phone's
// screen, every link on it answers HTTP 200 (the GitHub link is not fetched), the guide's and the API docs' front pages load
// with no console error, and each card's game reaches its world (the page's `data-torque-figure`) with no console error,
// page error or failed request. A card that opens a map (`?map=`) goes straight to it; any other waits at its game's menu
// (no figure while the menu is up) until the check presses Play alone (by script: the vehicle demo's `playAlone`, the
// arena's `joinPage`); its button back to the gallery is shown and leads there. Pictures of the gallery at a desktop's and
// a phone's width go into the pictures folder.
//
// Usage: node site-check.mjs [site] [pictures] [seconds per game]
// (default: <target>/site, <target>/site-check, 120). Needs puppeteer-core in the folder `TORQUE_WEB_TOOLS` names (default:
// <target>/web-tools/node, where the justfiles install it) and Chrome. Every page carries `?nolock` and the browser is
// guard.mjs's: no page can take the pointer lock.
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { serve } from './serve.mjs';
import { launchGuarded, noLock } from './guard.mjs';

const PREFIX = '/torque-rs/';
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const target = resolve(process.env.CARGO_TARGET_DIR ?? join(root, 'target'));
const [site = join(target, 'site'), pictures = join(target, 'site-check'), seconds = '120'] = process.argv.slice(2);
process.env.TORQUE_WEB_TOOLS ??= join(target, 'web-tools', 'node');
mkdirSync(pictures, { recursive: true });

const server = await serve(site, 0, PREFIX);
const origin = `http://127.0.0.1:${server.address().port}`;
const front = `${origin}${PREFIX}`;
const problems = [];
const browser = await launchGuarded({
  args: ['--enable-unsafe-webgpu', '--enable-gpu', '--ignore-gpu-blocklist', '--window-size=1400,900'],
  defaultViewport: { width: 1400, height: 900 },
});

// A page whose console errors, page errors, failed requests, error answers and requests off the server are `problems`
// under `label`.
async function watched(label) {
  const page = await browser.newPage();
  const say = (text) => problems.push(`${label}: ${text}`);
  page.on('console', (m) => m.type() === 'error' && say(`console: ${m.text()}`));
  page.on('pageerror', (e) => say(`page error: ${e.message}`));
  page.on('requestfailed', (r) => say(`request failed: ${r.url()} ${r.failure()?.errorText}`));
  page.on('request', (r) => {
    const url = r.url();
    if (!url.startsWith(origin) && !url.startsWith('data:')) say(`a request off the server: ${url}`);
  });
  page.on('response', (r) => r.status() >= 400 && say(`HTTP ${r.status()}: ${r.url()}`));
  return page;
}

try {
  // The gallery: its links, and its pictures at two widths.
  const gallery = await watched('gallery');
  await gallery.goto(noLock(front), { waitUntil: 'networkidle0' });
  const links = await gallery.$$eval('a[href]', (as) =>
    as.map((a) => ({ href: a.href, card: a.classList.contains('card'), text: a.textContent.trim().split('\n')[0] })),
  );
  const cards = links.filter((l) => l.card);
  console.log(`gallery: ${cards.length} cards, ${links.length} links`);
  if (cards.length === 0) problems.push('gallery: no cards');
  const broken = await gallery.$$eval('img', (imgs) => imgs.filter((i) => !(i.naturalWidth > 0)).map((i) => i.src));
  for (const src of broken) problems.push(`gallery: a picture did not load: ${src}`);
  writeFileSync(join(pictures, 'gallery-desktop.png'), await gallery.screenshot({ type: 'png', fullPage: true }));
  await gallery.setViewport({ width: 390, height: 844, deviceScaleFactor: 2, isMobile: true, hasTouch: true });
  await gallery.reload({ waitUntil: 'networkidle0' });
  const wide = await gallery.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  if (wide > 0) problems.push(`gallery: ${wide} px wider than a phone's screen`);
  writeFileSync(join(pictures, 'gallery-phone.png'), await gallery.screenshot({ type: 'png', fullPage: true }));
  await gallery.close();
  console.log(`pictures: ${join(pictures, 'gallery-desktop.png')}, ${join(pictures, 'gallery-phone.png')}`);

  for (const link of links) {
    if (!link.href.startsWith(origin)) {
      console.log(`not fetched (off the site): ${link.href}`);
      continue;
    }
    const response = await fetch(link.href);
    console.log(`HTTP ${response.status}: ${link.href}`);
    if (response.status !== 200) problems.push(`gallery: the link ${link.href} answers HTTP ${response.status}`);
  }

  // The guide's and the API docs' front pages.
  for (const path of ['book/', 'api/']) {
    const page = await watched(path);
    await page.goto(noLock(`${front}${path}`), { waitUntil: 'networkidle0' });
    console.log(`${path}: "${await page.title()}" at ${page.url()}`);
    await page.close();
  }

  // Each card's game, to its world.
  for (const card of cards) {
    const page = await watched(card.text);
    const started = Date.now();
    await page.goto(noLock(card.href), { waitUntil: 'load' });
    // its button back to the gallery, shown, leads to the gallery
    const back = await page.$eval('#gallery', (a) => (a.hidden ? 'hidden' : a.href)).catch(() => 'missing');
    if (back !== front) problems.push(`${card.text}: the gallery button is ${back.startsWith('http') ? `at ${back}` : back}`);
    // a click on it asks first and stays on the page; Stay closes the question
    if (back === front) {
      await page.evaluate(() => document.getElementById('gallery').click());
      const asked = await page.evaluate(() => !document.getElementById('leave').hidden && location.href);
      if (asked !== page.url()) problems.push(`${card.text}: the gallery button left without asking`);
      await page.evaluate(() => document.querySelector('#leave button').click());
      if (!(await page.$eval('#leave', (d) => d.hidden))) problems.push(`${card.text}: Stay left the question open`);
    }
    const deadline = started + Number(seconds) * 1000;
    const menuFirst = !new URL(card.href).searchParams.has('map');
    let menuSince = null;
    let pressed = false;
    let s = {};
    const before = problems.length;
    while (Date.now() < deadline && problems.length === before) {
      s = await page.evaluate(() => ({
        figure: document.documentElement.dataset.torqueFigure === '1',
        frames: Number(document.documentElement.dataset.torqueFrames ?? 0),
        map: document.documentElement.dataset.torqueMap ?? '',
        stage: document.documentElement.dataset.torqueStage ?? '',
        status: document.getElementById('status')?.textContent ?? '',
      }));
      if (menuFirst && !pressed) {
        if (s.figure) {
          problems.push(`${card.text}: the game started before Play alone was pressed`);
          break;
        }
        // The menu, drawn and held for three seconds: Play alone.
        if (s.stage === 'ready' && s.frames > 30) menuSince ??= Date.now();
        if (menuSince !== null && Date.now() - menuSince > 3000) {
          const url = new URL(card.href);
          const file = join(pictures, `${url.pathname.split('/').filter(Boolean).pop()}-menu.png`);
          writeFileSync(file, await page.screenshot({ type: 'png' }));
          console.log(`${card.text}: at its menu: ${file}`);
          await page.evaluate(() => globalThis.torque.call('playAlone').catch(() => globalThis.torque.call('joinPage')));
          pressed = true;
        }
      }
      if (s.figure) break;
      await new Promise((r) => setTimeout(r, 250));
    }
    const took = ((Date.now() - started) / 1000).toFixed(1);
    if (s.figure) {
      // A few more frames drawn with the figure held: the game runs on.
      const at = s.frames;
      const until = Date.now() + 30_000;
      while (Date.now() < until && problems.length === before) {
        const frames = await page.evaluate(() => Number(document.documentElement.dataset.torqueFrames ?? 0));
        if (frames >= at + 60) break;
        await new Promise((r) => setTimeout(r, 250));
      }
      const url = new URL(card.href);
      const map = url.searchParams.get('map');
      const file = join(pictures, `${url.pathname.split('/').filter(Boolean).pop()}${map ? `-${map}` : ''}.png`);
      writeFileSync(file, await page.screenshot({ type: 'png' }));
      console.log(`${card.text}: in the world in ${took} s (map "${s.map}"): ${file}`);
    } else {
      problems.push(`${card.text}: no figure within ${seconds} s (frames ${s.frames}, map "${s.map}", "${s.status}")`);
    }
    await page.close();
  }
} finally {
  await browser.close();
  server.close();
}
if (problems.length) {
  console.error('FAILED:\n  ' + problems.join('\n  '));
  process.exit(1);
}
console.log('ok: the gallery, its links and its games, with no console error');
