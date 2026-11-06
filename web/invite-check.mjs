// The arena joined by an invite link, on its page played alone (`node build.mjs fps --solo`), served from 127.0.0.1 with
// no game server: the host page makes an invite link, a second browser opens it and makes a reply link, the reply link
// is opened in the host's browser (it hands the reply to the game's tab and closes), the data channel opens, the joiner
// walks, each page holds the other's figure, and no console error shows. Then bad, cut short, oversized and used codes
// must each be refused with a message.
//
// Usage: node invite-check.mjs <solo web build folder> <pictures folder> [seconds]
// The invites use the same-network server: host candidates only, no STUN server is contacted (the two browsers reach each
// other on this computer's own addresses, shown as addresses rather than `.local` names). The browser comes from
// `guard.mjs` (TORQUE_WEB_TOOLS and CHROME as it reads them) and every page loads with `?nolock`: no page can take the
// pointer lock. The check presses keys only; it never moves or clicks the mouse (buttons are pressed by script).
import { mkdirSync } from 'node:fs';
import { writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { launchGuarded, noLock } from './guard.mjs';
import { serve } from './serve.mjs';
import { KIND_JOIN, encodeCode } from './codes.js';

const [folder, shots, seconds = '120'] = process.argv.slice(2);
if (!folder || !shots) {
  console.error('usage: node invite-check.mjs <solo web build folder> <pictures folder> [seconds]');
  process.exit(2);
}
mkdirSync(shots, { recursive: true });
const shot = (name) => join(resolve(shots), name);

const problems = [];
const log = [];
const browsers = [];
const launch = async () => {
  const browser = await launchGuarded({
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
  browsers.push(browser);
  return browser;
};

async function watched(browser, name) {
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
  page.on('request', (r) => {
    if (!r.url().startsWith(base) && !r.url().startsWith('data:')) problems.push(`${name} asked outside the page: ${r.url()}`);
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
      join: r.dataset.torqueJoin ?? '',
      reply: r.dataset.torqueReply ?? '',
      invites: r.dataset.torqueInvites ?? '',
      said: [...document.querySelectorAll('#invites [data-said]')].map((e) => e.textContent).join(' | '),
      links: [...document.querySelectorAll('#invites input[data-link]')].map((e) => e.value),
      lines: [...document.querySelectorAll('#invites li > p')].map((e) => e.textContent),
      hash: location.hash,
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

// Presses the panel's button by its text (by script: no mouse).
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

const panelShot = async (page, name) => {
  const panel = await page.$('#invites');
  if (!panel) return problems.push(`no panel for ${name}`);
  await writeFile(shot(name), await panel.screenshot({ type: 'png' }));
  console.log(`picture: ${shot(name)}`);
};

// A page of browser `browser` opened on `link` must refuse it, saying `pattern`.
async function refusedOn(browser, name, link, field, pattern) {
  const page = await watched(browser, name);
  await page.goto(noLock(link), { waitUntil: 'load' });
  const s = await wait(page, 20_000, (s) => s[field] === 'refused');
  if (s[field] !== 'refused' || !pattern.test(s.said)) problems.push(`${name}: not refused as it should be ("${s.said}")`);
  else console.log(`${name}: refused: "${s.said}"`);
  if (s.hash) problems.push(`${name}: the fragment stayed in the address bar`);
  await page.close();
}

let server;
let base;
try {
  server = await serve(resolve(folder), 0);
  base = `http://127.0.0.1:${server.address().port}/`;
  console.log(`page at ${base}`);
  const hostBrowser = await launch();
  const joinBrowser = await launch();
  const host = await watched(hostBrowser, 'host');
  await host.goto(noLock(`${base}?name=Host`), { waitUntil: 'load' });
  let h = await wait(host, Number(seconds) * 1000, (s) => s.figure);
  if (!h.figure) problems.push(`the host page has no figure (status "${h.status}")`);

  // The menu's Host with a link: the panel, its privacy text shown before any link is made.
  await host.evaluate(() => globalThis.torqueAsk('host'));
  await panelShot(host, 'host-panel.png');
  const privacy = await host.evaluate(() => document.querySelector('#invites .privacy')?.textContent ?? '');
  if (!/Cloudflare's free public STUN server/.test(privacy)) problems.push(`the host panel's privacy text: "${privacy}"`);
  await press(host, 'Same network only');
  await press(host, 'Make an invite link');
  h = await wait(host, 20_000, (s) => s.links.length > 0 || /failed/.test(s.invites));
  const invite = h.links[0];
  if (!invite) problems.push(`no invite link (${h.invites}: ${h.said})`);
  else console.log(`invite link: ${invite.length} characters, code ${invite.split('#join=')[1].length}`);
  await panelShot(host, 'host-invite.png');

  // The joiner opens it.
  const joiner = await watched(joinBrowser, 'joiner');
  await joiner.goto(noLock(`${invite}`.replace('#', '?name=Joiner#')), { waitUntil: 'load' });
  let j = await wait(joiner, 30_000, (s) => s.join === 'ready' || s.join === 'refused');
  if (j.hash) problems.push('the invite stayed in the joiner\'s address bar');
  if (j.join !== 'ready') problems.push(`the joiner's page did not take the invite ("${j.said}")`);
  await panelShot(joiner, 'join-panel.png');
  await press(joiner, 'Connect');
  j = await wait(joiner, 30_000, (s) => s.links.length > 0 || s.join === 'refused');
  const replyLink = j.links[0];
  if (!replyLink) problems.push(`no reply link ("${j.said}")`);
  else console.log(`reply link: ${replyLink.length} characters, code ${replyLink.split('#reply=')[1].length}`);
  await panelShot(joiner, 'join-reply.png');

  // The host opens the reply link in its own browser: that tab hands it over and closes.
  if (!problems.length) {
    const tab = await watched(hostBrowser, 'reply tab');
    const closed = new Promise((ok) => tab.once('close', ok));
    // opened as from a chat app: the tab's only page (its blank first page replaced), so it may close itself
    await Promise.all([tab.waitForNavigation({ waitUntil: 'load' }), tab.evaluate((u) => location.replace(u), noLock(replyLink))]);
    const t = await wait(tab, 10_000, (s) => s.reply !== '').catch(() => ({ reply: 'taken', said: '(closed)' }));
    if (t.reply !== 'taken') problems.push(`the reply tab: ${t.reply} ("${t.said}")`);
    const gone = await Promise.race([closed.then(() => true), new Promise((r) => setTimeout(() => r(false), 5000))]);
    console.log(`reply tab: ${t.reply}, closed itself: ${gone}`);
    if (!gone) problems.push('the reply tab did not close');
    await host.bringToFront();
  }

  // The channel opens: each holds the other, and the joiner walks.
  if (!problems.length) {
    j = await wait(joiner, Number(seconds) * 1000, (s) => (s.figure && s.link === 'datachannel' && s.others >= 1) || s.ended);
    console.log(`joiner: link ${j.link}, figure ${j.figure}, other figures ${j.others}, ended "${j.ended}"`);
    if (!j.figure || j.others < 1 || j.link !== 'datachannel') problems.push(`the joiner is not in the host's game (${JSON.stringify(j)})`);
    h = await wait(host, 20_000, (s) => s.others >= 1);
    console.log(`host: other figures ${h.others}, invites "${h.invites}"`);
    if (h.others < 1) problems.push("the host page does not hold the joiner's figure");
    if (!/joined/.test(h.invites)) problems.push(`the host's invite is not marked joined ("${h.invites}": ${h.lines.join(' | ')})`);
  }
  if (!problems.length) {
    const canvas = await joiner.$('canvas');
    await writeFile(shot('joined.png'), await canvas.screenshot({ type: 'png' }));
    const from = j.position;
    await canvas.focus();
    await joiner.keyboard.down('KeyW');
    await new Promise((r) => setTimeout(r, 2000));
    await joiner.keyboard.up('KeyW');
    const after = await wait(joiner, 30_000, (t) => t.frames >= j.frames + 150);
    const moved = Math.hypot(after.position[0] - from[0], after.position[1] - from[1]);
    console.log(`W held 2 s: the joiner walked ${moved.toFixed(2)}`);
    if (!(moved > 1)) problems.push(`the joiner did not walk (moved ${moved})`);
    await writeFile(shot('host-sees-joiner.png'), await (await host.$('canvas')).screenshot({ type: 'png' }));
  }

  // A Cloudflare invite (made here, never connected): the joiner's panel names the server before anything is sent.
  if (!problems.length) {
    const code = await encodeCode({
      kind: KIND_JOIN,
      server: 'cloudflare',
      token: 'ShownOnly-01',
      ufrag: 'abcd',
      pwd: 'abcdefghijklmnopqrstuvwx',
      fingerprint: new Uint8Array(32),
      setup: 'actpass',
      sctpPort: 5000,
      candidates: [{ type: 'srflx', ip: '203.0.113.7', port: 50000 }],
    });
    const page = await watched(joinBrowser, 'cloudflare invite');
    await page.goto(noLock(`${base}#join=${code}`), { waitUntil: 'load' });
    const s = await wait(page, 20_000, (s) => s.join === 'ready');
    const text = await page.evaluate(() => document.getElementById('invites')?.textContent ?? '');
    if (s.join !== 'ready' || !/Cloudflare's free public STUN server/.test(text)) problems.push(`the Cloudflare invite's panel: "${text}"`);
    await panelShot(page, 'join-panel-cloudflare.png');
    await page.close();
  }

  // Refusals, each with a message.
  if (!problems.length) {
    const code = invite.split('#join=')[1];
    const page = invite.split('#')[0];
    await refusedOn(joinBrowser, 'bad code', `${page}#join=${code.slice(0, 30)}${code[30] === 'Q' ? 'R' : 'Q'}${code.slice(31)}`, 'join', /damaged|cut short/);
    await refusedOn(joinBrowser, 'cut short', `${page}#join=${code.slice(0, -12)}`, 'join', /damaged|cut short/);
    await refusedOn(joinBrowser, 'oversized', `${page}#join=${'A'.repeat(1500)}`, 'join', /too long/);
    await refusedOn(joinBrowser, 'not a code', `${page}#join=%3Cb%3Ehi%3C/b%3E`, 'join', /characters a code never has/);
    await refusedOn(joinBrowser, 'invite used again', invite, 'join', /already used/);
    await refusedOn(joinBrowser, 'reply as invite', `${page}#join=${replyLink.split('#reply=')[1]}`, 'join', /reply link, not an invite/);
    await refusedOn(hostBrowser, 'reply used again', replyLink, 'reply', /already used/);
    await host.bringToFront();
    // and pasted into the host's panel
    await host.evaluate((text) => {
      const input = document.querySelector('#invites input[placeholder="Paste a reply link"]');
      input.value = text;
    }, `${replyLink.slice(0, -5)}`);
    await press(host, 'Add');
    h = await wait(host, 10_000, (s) => /damaged|cut short/.test(s.said));
    if (!/damaged|cut short/.test(h.said)) problems.push(`a cut reply pasted into the host's panel: "${h.said}"`);
    else console.log(`pasted cut reply: refused: "${h.said}"`);
    // the game goes on
    h = await wait(host, 5000, (s) => s.others >= 1);
    if (h.others < 1) problems.push('the joiner left the host after the refusals');
  }
} catch (e) {
  problems.push(e.stack ?? e.message);
} finally {
  for (const b of browsers) await b.close().catch(() => {});
  server?.close();
}
if (problems.length) {
  console.error('FAILED:\n  ' + problems.join('\n  '));
  console.error('console:\n  ' + log.slice(0, 300).join('\n  '));
  process.exit(1);
}
console.log('ok: no console errors');
process.exit(0);
