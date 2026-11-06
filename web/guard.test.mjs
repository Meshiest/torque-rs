// The guard's own test: a page that asks for the pointer lock when it is clicked (on the body, on a canvas, with the
// `unadjustedMovement` option, from an iframe, and after trying to put the real method back) never gets one, in a page opened
// through `launchGuarded`: no `pointerlockchange`, no `pointerlockerror` and no `pointerLockElement`.
// Usage: node guard.test.mjs [browser]      (puppeteer-core in the folder TORQUE_WEB_TOOLS names; `browser` as launchGuarded
// takes it, default Chrome)
import { createServer } from 'node:http';
import { launchGuarded } from './guard.mjs';

const frame = `<!doctype html><canvas id=c></canvas><script>
  document.addEventListener('click', () => c.requestPointerLock());
  parent.postMessage('frame-ready', '*');
</script>`;
// a popup reports, from its very first script, whether it still has the native method
const popup = `<!doctype html><script>
  opener?.postMessage({ native: /native code/.test(Function.prototype.toString.call(Element.prototype.requestPointerLock)) }, '*');
</script>`;
const page = `<!doctype html><meta charset=utf-8><body style="margin:0"><canvas id=canvas width=300 height=200></canvas>
<iframe id=f src="/frame" width=200 height=100></iframe>
<script>
  window.events = [];
  window.popupMessages = [];
  addEventListener('message', (e) => popupMessages.push(e.data));
  for (const type of ['pointerlockchange', 'pointerlockerror']) document.addEventListener(type, () => events.push(type));
  window.attempts = 0;
  const ask = () => {
    attempts++;
    const unlock = [];
    // the lock, as a page would take it after its click
    try { canvas.requestPointerLock(); } catch (e) { unlock.push('canvas ' + e); }
    try { document.body.requestPointerLock({ unadjustedMovement: true }); } catch (e) { unlock.push('body ' + e); }
    try { document.documentElement.requestPointerLock(); } catch (e) { unlock.push('root ' + e); }
    // and attempts to bring the real method back
    const real = { requestPointerLock() { events.push('real method called'); } }.requestPointerLock;
    for (const put of [
      () => Object.defineProperty(Element.prototype, 'requestPointerLock', { value: real }),
      () => Object.defineProperty(document.body, 'requestPointerLock', { value: real }),
      () => Object.defineProperties(canvas, { requestPointerLock: { value: real } }),
      () => Reflect.defineProperty(canvas, 'requestPointerLock', { value: real }),
      () => { 'use strict'; canvas.requestPointerLock = real; },
    ]) { try { put(); } catch {} }
    try { canvas.requestPointerLock(); } catch {}
  };
  document.addEventListener('click', ask);
</script>`;

const server = createServer((req, res) => {
  res.setHeader('content-type', 'text/html');
  res.end(req.url.startsWith('/frame') ? frame : req.url.startsWith('/popup') ? popup : page);
}).listen(0, '127.0.0.1');
await new Promise((ok) => server.once('listening', ok));
const url = `http://127.0.0.1:${server.address().port}/`;

const browser = await launchGuarded({ browser: process.argv[2], defaultViewport: { width: 800, height: 600 } });
const problems = [];
const native = (tab) =>
  tab.evaluate(() => /native code/.test(Function.prototype.toString.call(Element.prototype.requestPointerLock)));
try {
  // the tab the browser opened at launch is guarded before anything loads in it.
  const first = (await browser.pages())[0];
  for (const make of [
    async () => first,
    () => browser.newPage(),
    async () => (await browser.createBrowserContext()).newPage(),
    () => browser.defaultBrowserContext().newPage(),
  ]) {
    const tab = await make();
    await tab.goto(url, { waitUntil: 'load' });
    await tab.click('#canvas');
    await tab.click('#canvas');
    await new Promise((ok) => setTimeout(ok, 500));
    const seen = await tab.evaluate(() => ({
      events: events.slice(),
      attempts,
      lockedTo: document.pointerLockElement?.tagName ?? null,
      native: /native code/.test(Function.prototype.toString.call(Element.prototype.requestPointerLock)),
    }));
    const inFrame = await tab.frames()[1]?.evaluate(() => ({ lockedTo: document.pointerLockElement?.tagName ?? null }));
    if (seen.attempts < 2) problems.push(`the page's click handler ran ${seen.attempts} times`);
    if (seen.events.length) problems.push(`the page saw ${seen.events.join(', ')}`);
    if (seen.lockedTo || inFrame?.lockedTo) problems.push(`an element holds the lock: ${seen.lockedTo ?? inFrame.lockedTo}`);
    if (seen.native) problems.push('the page still has the native requestPointerLock');
    if (tab !== first) await tab.close();
  }
  // Popups: `window.open` (a blank one, a page of ours, and a `target=_blank` link) must open nothing or open a guarded page.
  const opener = await browser.newPage();
  await opener.goto(url, { waitUntil: 'load' });
  const opened = await opener.evaluate(() => {
    const blank = window.open('about:blank');
    const ours = window.open('/popup');
    const link = document.createElement('a');
    link.href = '/popup?link';
    link.target = '_blank';
    document.body.append(link);
    link.click();
    return { blank: blank !== null, ours: ours !== null };
  });
  await new Promise((ok) => setTimeout(ok, 500));
  const firstScripts = await opener.evaluate(() => popupMessages);
  if (firstScripts.some((m) => m.native)) problems.push('a popup ran its first script before the guard');
  console.log(`first scripts of popups: ${JSON.stringify(firstScripts)}`);
  for (const tab of await browser.pages()) {
    if (await native(tab)) problems.push(`a page at ${tab.url()} has the native requestPointerLock (popups: ${JSON.stringify(opened)})`);
  }
  console.log(`popups opened: ${JSON.stringify(opened)}; pages open: ${(await browser.pages()).length}`);
} finally {
  await browser.close();
  server.close();
}
if (problems.length) {
  console.error(`FAILED:\n  ${problems.join('\n  ')}`);
  process.exit(1);
}
console.log('ok: a page that asks for the pointer lock on click never gets it');
