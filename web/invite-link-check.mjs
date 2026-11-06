// The invite links' connection on its own, with no game and no build: a small page runs `invites.js` as a page played
// alone does (the host's panel, the joiner's, the reply tab), with stand-ins for the game (the host's side echoes what
// the joiner sends, doubled). The host page makes an invite offer, the joiner's page opens it and presses Connect, and
// after `delay` seconds (a person relaying the reply link by hand) the reply link is opened in a new tab of the host's
// browser, which hands it to the host's panel. The data channel must open, a message must go each way, and no console
// error may show.
//
// Usage: node invite-link-check.mjs <host browser>[,<joiner browser>] [same|two] [delay]
// The browsers are names `guard.mjs`'s `launchGuarded` takes (as `browser`), each in its own headless instance with its
// own defaults (mDNS names for host candidates included). With `same` (the default) the two pages are tabs of ONE
// browser and the invite is on the default server (Cloudflare); with `two` they are two browsers (the same program twice,
// or the two named) and the invite is on Same network only. The delay is 30 s by default: longer than a joiner's browser
// waits before it gives up on a host it cannot reach yet.
// No STUN server is ever contacted: every page's `RTCPeerConnection` is given no ICE servers, whatever the page asks for
// (what it asked for is checked). The page is served from 127.0.0.1; buttons are pressed by script, never by the mouse.
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { launchGuarded, noLock } from './guard.mjs';

const [names = '', mode = 'same', delayText = '30'] = process.argv.slice(2);
const [hostName, joinName = hostName] = names.split(',');
const delay = Number(delayText);
if (!/^[a-z]+$/.test(hostName ?? '') || !/^[a-z]+$/.test(joinName) || !['same', 'two'].includes(mode) || !(delay >= 0)) {
  console.error('usage: node invite-link-check.mjs <host browser>[,<joiner browser>] [same|two] [delay]');
  process.exit(2);
}
if (mode === 'same' && joinName !== hostName) {
  console.error('`same` puts both pages in one browser: name one');
  process.exit(2);
}
const here = dirname(fileURLToPath(import.meta.url));

// The page: `invites.js` as `index.html` uses it on a page played alone, with stand-ins for the game.
const PAGE = `<!doctype html><meta charset=utf-8><title>Invite links</title><body><script type=module>
import { hostInvites, joinInvite, replyTab, takeFragment } from './invites.js';
const root = document.documentElement;
const found = takeFragment();
if (found?.kind === 'reply') replyTab(found.code);
else if (found?.kind === 'join') {
  const join = joinInvite(found.code);
  let send = null;
  join.attach({
    setLocalSender: (f) => (send = f),
    // sent until the echo comes back: the host's side takes the channel a moment after it opens
    linkOpened: () => {
      root.dataset.torqueLink = 'datachannel';
      const again = setInterval(() => (root.dataset.echo ? clearInterval(again) : send(new Uint8Array([1, 2, 3]))), 250);
    },
    linkEnded: (why) => (root.dataset.torqueEnded = why),
    localDatagram: (bytes) => (root.dataset.echo = [...bytes].join(' ')),
  });
} else {
  hostInvites({ open: true })({
    adopt(pc, ch) {
      ch.binaryType = 'arraybuffer';
      ch.onmessage = (e) => {
        const bytes = new Uint8Array(e.data);
        root.dataset.hostGot = [...bytes].join(' ');
        ch.send(bytes.map((b) => b * 2));
      };
      return null;
    },
  });
}
</script>`;
const FILES = new Set(['invites.js', 'codes.js', 'host.js']);

const problems = [];
const log = [];
const browsers = [];
// how many times each page's browser reported a connection's ICE failing
const iceFailed = { host: 0, joiner: 0, 'reply tab': 0 };
// Every page's peer connections get no ICE servers, so no STUN server is contacted; what the page asked for is kept.
function noStun() {
  const Real = globalThis.RTCPeerConnection;
  if (!Real) return;
  const asked = [];
  function Stunless(config = {}, ...rest) {
    asked.push((config?.iceServers ?? []).flatMap((s) => [].concat(s.urls ?? [])).join(' '));
    return new Real({ ...config, iceServers: [] }, ...rest);
  }
  Stunless.prototype = Real.prototype;
  Object.setPrototypeOf(Stunless, Real);
  globalThis.RTCPeerConnection = Stunless;
  globalThis.iceServersAsked = asked;
}

let base;
async function watched(browser, name) {
  const page = await browser.newPage();
  await page.evaluateOnNewDocument(noStun);
  page.on('console', (m) => {
    const line = `${name} ${m.type()}: ${m.text()}`;
    log.push(line);
    if (m.type() === 'error') problems.push(line);
  });
  page.on('pageerror', (e) => {
    // what Firefox reports each time a connection's ICE fails: the joiner's do while the reply is on its way
    if (/^WebRTC: ICE failed,/.test(e.message)) log.push(`${name} ice failed: ${e.message}`), iceFailed[name]++;
    else problems.push(`${name} page error: ${e.message}`);
  });
  page.on('request', (r) => {
    if (!r.url().startsWith(base) && !r.url().startsWith('data:')) problems.push(`${name} asked outside the page: ${r.url()}`);
  });
  return page;
}

const state = (page) =>
  page.evaluate(() => {
    const r = document.documentElement;
    return {
      join: r.dataset.torqueJoin ?? '',
      reply: r.dataset.torqueReply ?? '',
      invites: r.dataset.torqueInvites ?? '',
      link: r.dataset.torqueLink ?? '',
      ended: r.dataset.torqueEnded ?? '',
      echo: r.dataset.echo ?? '',
      hostGot: r.dataset.hostGot ?? '',
      said: [...document.querySelectorAll('#invites [data-said]')].map((e) => e.textContent).join(' | '),
      links: [...document.querySelectorAll('#invites input[data-link]')].map((e) => e.value),
      lines: [...document.querySelectorAll('#invites li > p')].map((e) => e.textContent),
      asked: globalThis.iceServersAsked ?? [],
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

// Presses the panel's button (or radio) by its text, by script.
async function press(page, text) {
  const pressed = await page.evaluate((text) => {
    const b = [...document.querySelectorAll('#invites button, #invites input[type=radio]')].find(
      (e) => e.textContent === text || e.labels?.[0]?.textContent === text,
    );
    b?.click();
    return Boolean(b);
  }, text);
  if (!pressed) problems.push(`no "${text}" on the panel`);
}

const server = createServer(async (req, res) => {
  const path = new URL(req.url, 'http://127.0.0.1').pathname.slice(1);
  if (path === '') {
    res.setHeader('content-type', 'text/html; charset=utf-8');
    return res.end(PAGE);
  }
  // the icon a browser asks for by itself: none
  if (path === 'favicon.ico') return res.writeHead(204).end();
  if (!FILES.has(path)) return res.writeHead(404).end();
  res.setHeader('content-type', 'text/javascript; charset=utf-8');
  res.end(await readFile(join(here, path)));
}).listen(0, '127.0.0.1');

try {
  await new Promise((ok) => server.once('listening', ok));
  base = `http://127.0.0.1:${server.address().port}/`;
  const launch = async (name) => {
    const browser = await launchGuarded({ browser: name });
    browsers.push(browser);
    return browser;
  };
  const hostBrowser = await launch(hostName);
  const joinBrowser = mode === 'same' ? hostBrowser : await launch(joinName);
  console.log(`${hostName} hosts, ${joinName} joins, ${mode === 'same' ? 'one browser' : 'two browsers'}, reply after ${delay} s`);

  const host = await watched(hostBrowser, 'host');
  await host.goto(noLock(base), { waitUntil: 'load' });
  await host.waitForSelector('#invites .privacy', { timeout: 20_000 });
  if (mode === 'two') await press(host, 'Same network only');
  await press(host, 'Create Invite Offer');
  let h = await wait(host, 20_000, (s) => s.links.length > 0 || /failed/.test(s.invites));
  const invite = h.links[0];
  if (!invite) throw new Error(`no invite link (${h.invites}: ${h.lines.join(' | ')} ${h.said})`);
  console.log(`invite link: code ${invite.split('#join=')[1].length} characters`);

  const joiner = await watched(joinBrowser, 'joiner');
  await joiner.goto(noLock(invite), { waitUntil: 'load' });
  let j = await wait(joiner, 20_000, (s) => s.join === 'ready' || s.join === 'refused');
  if (j.join !== 'ready') throw new Error(`the joiner's page did not take the invite ("${j.said}")`);
  await press(joiner, 'Connect');
  j = await wait(joiner, 20_000, (s) => s.links.length > 0 || s.join === 'refused');
  const replyLink = j.links[0];
  if (!replyLink) throw new Error(`no reply link ("${j.said}")`);
  console.log(`reply link: code ${replyLink.split('#reply=')[1].length} characters`);

  if (delay > 0) {
    console.log(`waiting ${delay} s before the reply link is opened`);
    await new Promise((r) => setTimeout(r, delay * 1000));
  }
  const taken = Date.now();
  const tab = await watched(hostBrowser, 'reply tab');
  await tab.goto(noLock(replyLink), { waitUntil: 'load' });
  const t = await wait(tab, 10_000, (s) => s.reply !== '').catch(() => ({ reply: 'taken', said: '(closed)' }));
  if (t.reply !== 'taken') throw new Error(`the reply tab: ${t.reply} ("${t.said}")`);

  j = await wait(joiner, 60_000, (s) => s.echo !== '' || s.ended !== '');
  h = await wait(host, 5_000, (s) => /joined|failed/.test(s.invites));
  console.log(`after ${((Date.now() - taken) / 1000).toFixed(1)} s: joiner "${j.join}", echo "${j.echo}"; host "${h.invites}", got "${h.hostGot}"`);
  if (j.join !== 'open' || j.link !== 'datachannel') problems.push(`the joiner did not connect ("${j.join}": ${j.said} ${j.ended})`);
  if (j.echo !== '2 4 6' || h.hostGot !== '1 2 3') problems.push(`the messages: host got "${h.hostGot}", joiner got "${j.echo}"`);
  if (!/joined/.test(h.invites)) problems.push(`the host's invite is not joined ("${h.invites}": ${h.lines.join(' | ')})`);
  console.log(`ICE failures reported: host ${iceFailed.host}, joiner ${iceFailed.joiner}`);
  if (iceFailed.host) problems.push(`the host's connection failed ${iceFailed.host} time(s)`);
  // what each page asked for: the invite's server (given none here)
  const want = mode === 'same' ? 'stun:stun.cloudflare.com:3478' : '';
  for (const [name, page] of [['host', host], ['joiner', joiner]]) {
    const asked = (await state(page)).asked;
    if (!asked.length || asked.some((a) => a !== want)) problems.push(`${name} asked for ICE servers "${asked.join(' | ')}", not "${want}"`);
    else console.log(`${name}: ${asked.length} connection(s), each asked for "${want}"`);
  }
} catch (e) {
  problems.push(e.stack ?? e.message);
} finally {
  for (const b of browsers) await b.close().catch(() => {});
  server.close();
}
if (problems.length) {
  console.error('FAILED:\n  ' + problems.join('\n  '));
  console.error('console:\n  ' + log.slice(0, 200).join('\n  '));
  process.exit(1);
}
console.log('ok: connected, a message each way, no console errors');
process.exit(0);
