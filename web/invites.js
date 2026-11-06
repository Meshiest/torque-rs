// Games joined by invite links, on a page with no server of ours (a solo page: `build.mjs --solo`). The host's page runs
// the game's server in its worker (host.js); the host makes one invite link per player (`<page>#join=<code>`, codes.js),
// each for one peer connection of its own, and sends it in a chat. The player's page opens it, makes its answer, and shows
// a reply link (`<page>#reply=<code>`) to send back. The host opens that link in the same browser: that tab hands the reply
// to the game's tab over a BroadcastChannel and closes (or the host pastes it into the game's panel). The data channel then
// opens straight between the two browsers, and the joiner's channel goes to the worker's relay like any other link.
//
// Neither page loads anything from a code but what codes.js reads strictly: the STUN server is a key into the page's own
// list (codes.js STUN_SERVERS), and each candidate goes through host.js's `candidateAllowed` (canonical IPv4, or a browser's
// mDNS name in its strict form, `<uuid v4>.local`, which only the browser resolves and only on its own network; private
// addresses only on the same-network server or with `?lan=1`; on the host also never its own addresses or names). The mDNS
// names are what let two tabs of one browser, or two computers on one network, reach each other on any server: their
// public address alone needs a router that loops it back, which most home routers do not.
// Nothing from a code or a message is written as HTML: every text goes in by `textContent` or an input's `value`. The
// fragment is cleared from the address bar as soon as it is read.
//
// A connection that does not open shows its details under the message, to copy into a report: the browser's family, how the
// connection's states went and counts of its candidates and candidate pairs by type and state (`connectionDetails`, from
// the connection's stats just before it is closed). They never hold an address, a port, an mDNS name, a ufrag or a code.
//
// Caps: at most MAX_INVITES invites wait at once, each for INVITE_MS; a reply has CONNECT_MS for its channel to open; one
// reply is read at a time; a code is refused whole when it breaks any of codes.js's caps; an invite's reply is taken once.
import {
  CodeError,
  DEFAULT_SERVER,
  KIND_JOIN,
  KIND_REPLY,
  STUN_SERVERS,
  candidateLine,
  codeFromFragment,
  codeFromPaste,
  decodeCode,
  encodeCode,
  fromSdp,
  iceServersFor,
  newToken,
  pickCandidates,
  sameDetails,
  serverFor,
  toSdp,
  withDetails,
} from './codes.js';
import { MDNS_NAME, addressAllowed, candidateAllowed, ipv4, meteredReceiver } from './host.js';

const MAX_INVITES = 8;
const INVITE_MS = 15 * 60_000;
const CONNECT_MS = 30_000;
// How long the joiner's page waits for the host to take its reply and the channel to open.
const JOIN_MS = 5 * 60_000;
const GATHER_MS = 5_000;
// The shortest time between two of a joiner's connections (see `again` in joinInvite).
const RETRY_MS = 4_000;
// A time in whole minutes for the player ("15 minutes", "1 minute").
export const minutes = (ms) => `${Math.round(ms / 60_000)} minute${Math.round(ms / 60_000) === 1 ? '' : 's'}`;
const HANDOFF_MS = 2_000;
const MAX_BUFFERED = 256 * 1024;
const USED_KEY = 'torque-invites-used';
const MAX_USED = 32;
const TAKEN = 'The reply is in: connecting to the player.';
const TRUST = 'Only send this link to people you trust. It contains your IP address.';
const NO_RELAY =
  'Could not connect. Check that the reply link was opened in the browser that made the invite offer, then try a new ' +
  "invite offer on the other connection server. Some networks can't connect without a relay server, which this page " +
  'does not use; players on one network can pick Same network only.';

// Whether private addresses may be used: the same-network server, or on the host's own page `?lan=1`. A joining page
// ignores `?lan`: its address came from the host, so only the invite's server decides.
export function privateAllowed(server, search, joining) {
  return server === 'lan' || (!joining && new URLSearchParams(search).has('lan'));
}
const lanFor = (server) => privateAllowed(server, location.search, false);
const channelName = () => `torque-invites ${location.pathname}`;
const pageLink = () => `${location.origin}${location.pathname}`;

// The code the page's fragment carries ({ kind, code }), with the fragment cleared from the address bar; or null.
export function takeFragment() {
  const found = codeFromFragment(location.hash);
  if (location.hash) history.replaceState(null, '', location.pathname + location.search);
  return found;
}

// ---- the page's panel, made of elements and text only

function el(tag, props = {}, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === 'text') e.textContent = v;
    else if (k === 'on') for (const [name, fn] of Object.entries(v)) e.addEventListener(name, fn);
    else if (k in e) e[k] = v;
    else e.setAttribute(k, v);
  }
  e.append(...children.filter(Boolean));
  return e;
}

let panel = null;
// `escape`: what Esc does on the panel (by default it closes it).
function openPanel(title, escape = closePanel) {
  panel?.remove();
  panel = el('div', { id: 'invites', role: 'dialog', 'aria-labelledby': 'invites-title' });
  panel.append(el('h2', { id: 'invites-title', text: title }));
  panel.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      e.stopPropagation();
      escape();
    }
  });
  document.body.append(panel);
  return panel;
}
function closePanel() {
  panel?.remove();
  panel = null;
  document.querySelector('canvas')?.focus();
}

// What the chosen STUN server means for the player's address, as a paragraph.
function privacyText(key, who) {
  const server = STUN_SERVERS[key];
  if (!server.urls.length) {
    return el('p', {
      class: 'privacy',
      text:
        who === 'The host'
          ? 'No outside server is used.'
          : 'No outside server is used. Only players on the same network as you can join.',
    });
  }
  return el(
    'p',
    { class: 'privacy' },
    `To connect, this page asks ${server.name}'s free public STUN server for this computer's public IP address, so ` +
      `${server.name} sees that address and when you connected. ${who} sees it too: it is in ` +
      `${who === 'The host' ? 'your reply link' : 'the link'}. `,
    el('a', { href: server.privacy, target: '_blank', rel: 'noopener noreferrer', text: `${server.name}'s privacy policy` }),
  );
}

// A line of text to copy, in a read-only field (marked with the attribute `mark`) with its Copy button, and the line that
// says how the copy went.
function copyRow(text, label, mark, what) {
  const input = el('input', { type: 'text', readOnly: true, value: text, 'aria-label': label, [mark]: '' });
  const said = el('span', { class: 'said' });
  const copy = el('button', {
    type: 'button',
    text: 'Copy',
    on: {
      click: async () => {
        try {
          await navigator.clipboard.writeText(text);
          said.textContent = 'Copied.';
        } catch {
          input.select();
          said.textContent = `Select the ${what} and copy it.`;
        }
      },
    },
  });
  return { row: el('div', { class: 'row' }, input, copy), said };
}

// A link with its copy button and the trust warning.
function linkBox(link, label) {
  const { row, said } = copyRow(link, label, 'data-link', 'link');
  return el('div', { class: 'link' }, row, el('p', { class: 'warn', text: TRUST }), said);
}

// A failed connection's details (`connectionDetails`) with their copy button.
function detailsBox(details) {
  const { row, said } = copyRow(details, 'Connection details', 'data-details', 'details');
  return el(
    'div',
    { class: 'details' },
    el('p', { text: 'If you report this, send these connection details too. They hold no addresses.' }),
    row,
    said,
  );
}

// ---- a failed connection's details

const CANDIDATE_TYPES = ['host', 'srflx', 'prflx', 'relay'];
const PAIR_STATES = ['succeeded', 'failed', 'in-progress', 'waiting', 'frozen'];
const CONNECTION_STATES = ['new', 'checking', 'connecting', 'connected', 'completed', 'disconnected', 'failed', 'closed'];
const MAX_STATES = 16;

// The browser's family by its user agent, as the engine its WebRTC comes from: WebKit (Safari, and every browser on an
// iPhone or iPad), Gecko (Firefox), Edge, Blink (Chrome and the other Chromium browsers), or other. A user agent is read by
// its engine tokens: the scripts here name no browser product (web/guard-scan.mjs).
export function browserFamily(userAgent) {
  const ua = String(userAgent ?? '');
  if (/\b(iPhone|iPad|iPod|CriOS|FxiOS|EdgiOS)\b/.test(ua)) return 'WebKit';
  if (/\bGecko\/\d/.test(ua)) return 'Gecko';
  if (/\bEdgA?\//.test(ua)) return 'Edge';
  if (/\bAppleWebKit\/537\.36\b/.test(ua)) return 'Blink';
  if (/\bAppleWebKit\//.test(ua)) return 'WebKit';
  return 'other';
}

// The connection's states as they change, ICE's (`ice`) and the whole connection's (`peer`), for its details; with `seen`,
// added to an earlier connection's (one that this one took over from).
function watchStates(pc, seen = { ice: [], peer: [] }) {
  const add = (list, state) => list.length < MAX_STATES && list.at(-1) !== state && list.push(state);
  add(seen.ice, pc.iceConnectionState);
  add(seen.peer, pc.connectionState);
  pc.addEventListener('iceconnectionstatechange', () => add(seen.ice, pc.iceConnectionState));
  pc.addEventListener('connectionstatechange', () => add(seen.peer, pc.connectionState));
  return seen;
}

// One line of a failed connection's details, from the browser's user agent, the states `seen` (`watchStates`) and the
// connection's stats (a list of `getStats` entries): the browser's family, each state history, the local and remote
// candidates by type (host ones as mDNS names or addresses; a remote one with no address is unresolved: Chrome reports an
// mDNS name it could not resolve so) and the candidate pairs by state. Counts and the browser's own state words only:
// nothing of an address, port, name, ufrag or code, whatever the stats hold.
export function connectionDetails(userAgent, seen, stats) {
  const word = (v, known) => (known.includes(v) ? v : 'other');
  const count = (map, key) => map.set(key, (map.get(key) ?? 0) + 1);
  const sides = { local: { types: new Map(), host: new Map() }, remote: { types: new Map(), host: new Map() } };
  const pairs = new Map();
  let unresolved = 0;
  for (const s of stats ?? []) {
    if (s?.type === 'candidate-pair') {
      count(pairs, word(s.state, PAIR_STATES));
      continue;
    }
    const side = s?.type === 'local-candidate' ? sides.local : s?.type === 'remote-candidate' ? sides.remote : null;
    if (!side) continue;
    const type = word(s.candidateType, CANDIDATE_TYPES);
    count(side.types, type);
    const address = String(s.address ?? s.ip ?? '');
    if (!address) {
      if (side === sides.remote) unresolved++;
    } else if (type === 'host') {
      count(side.host, address.toLowerCase().endsWith('.local') ? 'mDNS' : 'address');
    }
  }
  const list = (map, order, each) => {
    const keys = [...order, 'other'].filter((k) => map.has(k));
    return keys.length ? keys.map(each).join(', ') : 'none';
  };
  const candidates = (side) =>
    list(side.types, CANDIDATE_TYPES, (t) => {
      if (t !== 'host' || !side.host.size) return `${side.types.get(t)} ${t}`;
      return `${side.types.get(t)} host (${list(side.host, ['mDNS', 'address'], (k) => `${side.host.get(k)} ${k}`)})`;
    });
  const history = (states) =>
    Array.isArray(states) && states.length
      ? states.slice(0, MAX_STATES).map((v) => word(v, CONNECTION_STATES)).join('>')
      : 'none';
  return [
    browserFamily(userAgent),
    `ice ${history(seen?.ice)}`,
    `connection ${history(seen?.peer)}`,
    `local ${candidates(sides.local)}`,
    `remote ${candidates(sides.remote)}${unresolved ? `, ${unresolved} unresolved` : ''}`,
    `pairs ${list(pairs, PAIR_STATES, (k) => `${pairs.get(k)} ${k}`)}`,
  ].join('; ');
}

// The details of `pc`, gathered before it is closed (a closed connection has no stats).
async function detailsOf(pc, seen) {
  let stats = [];
  try {
    stats = [...(await pc.getStats()).values()];
  } catch {}
  return connectionDetails(navigator.userAgent, seen, stats);
}

// Resolves once ICE gathering is complete, or after `ms`.
function gathered(pc, ms) {
  return new Promise((ok) => {
    if (pc.iceGatheringState === 'complete') return ok();
    const t = setTimeout(ok, ms);
    pc.addEventListener('icegatheringstatechange', () => {
      if (pc.iceGatheringState === 'complete') {
        clearTimeout(t);
        ok();
      }
    });
  });
}

const sdpLines = (sdp) => sdp.split(/\r?\n/).filter((l) => l.startsWith('a=candidate:'));

// The code's candidates as lines the browser may take (each through `candidateAllowed`, mDNS names included).
function allowedLines(data, lan, own) {
  const lines = [];
  data.candidates.forEach((c, i) => {
    const taken = candidateAllowed(candidateLine(c, i), lan, own, { mdns: true });
    if (taken) lines.push(taken.line);
  });
  return lines;
}

// The address the connection's traffic comes from (the selected pair's remote candidate), or null.
async function remoteAddress(pc) {
  const stats = await pc.getStats();
  let pair = null;
  stats.forEach((s) => {
    if (s.type === 'transport' && s.selectedCandidatePairId) pair = stats.get(s.selectedCandidatePairId);
  });
  if (!pair) stats.forEach((s) => s.type === 'candidate-pair' && s.selected && (pair = s));
  const remote = pair && stats.get(pair.remoteCandidateId);
  return remote?.address ?? remote?.ip ?? null;
}

const waitOpen = (ch, ms) =>
  new Promise((ok) => {
    if (ch.readyState === 'open') return ok(true);
    const t = setTimeout(() => ok(false), ms);
    ch.addEventListener('open', () => (clearTimeout(t), ok(true)));
    ch.addEventListener('close', () => (clearTimeout(t), ok(false)));
  });

const message = (e) => (e instanceof CodeError ? e.message : `Something went wrong: ${String(e?.message ?? e)}`);

// The page again with no invite or reply: its game played alone, at its menu.
function backToMenu() {
  location.replace(location.pathname + location.search);
}
const backButton = () => el('button', { type: 'button', class: 'close', text: 'Back to the menu', on: { click: backToMenu } });

// ---- the host

// A paste field for an invite link that opens the page again, joining.
function showJoinPaste() {
  const p = openPanel('Join with Link');
  const paste = el('input', { type: 'text', 'aria-label': 'An invite offer link', placeholder: 'Paste an invite offer link' });
  const said = el('p', { class: 'said', 'data-said': '' });
  const go = el('button', {
    type: 'button',
    class: 'primary',
    text: 'Join',
    on: {
      click: async () => {
        const code = codeFromPaste(paste.value, KIND_JOIN);
        if (code === null) {
          said.textContent = 'This is not an invite offer link.';
          return;
        }
        try {
          await decodeCode(code, KIND_JOIN);
        } catch (e) {
          said.textContent = message(e);
          return;
        }
        // the page again, joining: this game ends
        location.hash = `join=${code}`;
        location.reload();
      },
    },
  });
  p.append(
    el('p', { text: 'Paste the invite offer link a friend sent you. Your own game here ends when you join theirs.' }),
    el('div', { class: 'row' }, paste, go),
    said,
    el('button', { type: 'button', class: 'close', text: 'Close', on: { click: closePanel } }),
  );
  paste.focus();
}

// A game played alone in the page (no server in a worker): its menu's Host with a link opens the page again hosting
// (`?invites=1`), and Join with Link the paste field.
export function soloAsks() {
  globalThis.torqueAsk = (what) => {
    if (what === 'join') return showJoinPaste();
    if (what !== 'host') return;
    const url = new URL(location.href);
    url.hash = '';
    url.searchParams.set('invites', '1');
    location.assign(url.href);
  };
}

// The host's invites, for host.js's `onHost`: the menu's Host with a link opens its panel (`torqueAsk('host')`), and
// Join with Link a paste field that reloads the page into joining. `open`: the panel opens once the server runs.
export function hostInvites({ open = false } = {}) {
  let host = null;
  const invites = new Map();
  const used = new Set();
  // invites that expired or failed
  const closed = new Set();
  let count = 0;
  let busy = false;
  let server = DEFAULT_SERVER;
  let list = null;
  let said = null;
  const root = document.documentElement;

  const pending = () => [...invites.values()].filter((i) => i.state === 'waiting' || i.state === 'making').length;

  // every invite made, for the page's `data-torque-invites` (each one's state, oldest first)
  const made = [];
  // An invite's link is shown only while it waits for its reply: once the reply is in, or the invite expired or failed,
  // its link box goes (its status line stays).
  function setState(inv, state, text) {
    inv.state = state;
    inv.status.textContent = `Invite ${inv.n}: ${text}`;
    if (state !== 'waiting') inv.box?.remove();
    root.dataset.torqueInvites = made.map((i) => i.state).join(' ');
  }

  function forget(inv) {
    clearTimeout(inv.timer);
    invites.delete(inv.token);
    closed.add(inv.token);
  }

  async function makeInvite() {
    if (pending() >= MAX_INVITES) {
      said.textContent = `${MAX_INVITES} invite offers are waiting already. Wait for a reply or for one to expire.`;
      return;
    }
    const key = server;
    const inv = { n: ++count, token: newToken(), server: key, state: 'making', own: new Set(), status: el('p') };
    invites.set(inv.token, inv);
    if (made.length < 64) made.push(inv);
    const item = el('li', {}, inv.status);
    list.prepend(item);
    setState(inv, 'making', 'making the link...');
    try {
      inv.pc = new RTCPeerConnection({ iceServers: iceServersFor(key) });
      inv.seen = watchStates(inv.pc);
      inv.ch = inv.pc.createDataChannel('game', { ordered: false, maxRetransmits: 0 });
      inv.pc.onicecandidate = (e) => {
        const address = e.candidate?.candidate?.split(' ')[4];
        if (address) inv.own.add(address);
      };
      await inv.pc.setLocalDescription(await inv.pc.createOffer());
      await gathered(inv.pc, GATHER_MS);
      const sdp = inv.pc.localDescription.sdp;
      for (const line of sdpLines(sdp)) inv.own.add(line.split(' ')[4]);
      const candidates = pickCandidates(sdpLines(sdp), (o) => addressAllowed(o, lanFor(key)), { mdns: true });
      if (!candidates.length) {
        throw new CodeError(
          key === 'lan'
            ? 'This computer has no network address to put in the link.'
            : `Could not get this computer's public address from ${STUN_SERVERS[key].name}'s server. Try the other ` +
              'connection server, or Same network only.',
        );
      }
      // only this computer's network names: the server did not answer, so only players on this network can join
      const nearOnly = key !== 'lan' && !candidates.some((c) => c.type !== 'host');
      const code = await encodeCode({ kind: KIND_JOIN, server: key, token: inv.token, ...fromSdp(sdp), candidates });
      if (!invites.has(inv.token)) return;
      inv.timer = setTimeout(() => {
        if (inv.state !== 'waiting') return;
        inv.pc.close();
        forget(inv);
        setState(inv, 'expired', 'expired. Create a new one.');
      }, INVITE_MS);
      inv.box = linkBox(`${pageLink()}#join=${code}`, `Invite offer link ${inv.n}`);
      item.append(inv.box);
      setState(
        inv,
        'waiting',
        nearOnly
          ? `waiting for the player's reply. ${STUN_SERVERS[key].name}'s server did not answer, so only players on your ` +
              `network can use this one. Open for ${minutes(INVITE_MS)}.`
          : `waiting for the player's reply (through ${STUN_SERVERS[key].name}). Open for ${minutes(INVITE_MS)}.`,
      );
    } catch (e) {
      inv.pc?.close();
      forget(inv);
      setState(inv, 'failed', message(e));
    }
  }

  // Takes a reply (a link or a code, pasted or handed over): null when no invite of this page is its, else the text for
  // the player (TAKEN once the reply is in; the invite's line then says how the connection went).
  async function takeReply(text) {
    const code = codeFromPaste(text, KIND_REPLY);
    if (code === null) return 'This is not a reply link.';
    const data = await decodeCode(code, KIND_REPLY);
    if (used.has(data.token)) return 'This reply link was already used. To let this player in again, create a new invite offer.';
    if (closed.has(data.token)) return 'That invite offer is no longer open. Create a new one.';
    const inv = invites.get(data.token);
    if (!inv || inv.state !== 'waiting') return null;
    used.add(data.token);
    clearTimeout(inv.timer);
    setState(inv, 'connecting', 'connecting...');
    const { pc } = inv;
    try {
      await pc.setRemoteDescription({ type: 'answer', sdp: toSdp(data) });
      for (const line of allowedLines(data, lanFor(inv.server), inv.own)) {
        await pc.addIceCandidate({ candidate: line, sdpMid: '0' }).catch(() => {});
      }
    } catch {
      pc.close();
      forget(inv);
      setState(inv, 'failed', 'the reply does not fit this invite offer.');
      return 'The reply does not fit its invite offer.';
    }
    settle(inv);
    return TAKEN;
  }

  // The invite's channel opens in time and its player comes in, or it is closed.
  async function settle(inv) {
    const { pc, ch } = inv;
    if (!(await waitOpen(ch, CONNECT_MS))) {
      const details = await detailsOf(pc, inv.seen);
      pc.close();
      forget(inv);
      setState(inv, 'failed', NO_RELAY);
      inv.status.after(detailsBox(details));
      return;
    }
    // The address the joiner is counted by: where its traffic comes from, or, when the browser hides that (a peer-reflexive
    // pair: a joiner reached by its own checks, as one behind the host's own address is), a stand-in of this invite's own
    // in the reserved 240.0.0.0/8, so the other caps still hold.
    const seen = await remoteAddress(pc).catch(() => null);
    const ip = ipv4(seen) ? seen : `240.0.${(inv.n >> 8) & 255}.${inv.n & 255}`;
    const refused = host ? host.adopt(pc, ch, ip, () => setState(inv, 'left', 'left the game.')) : 'The game is not running.';
    if (refused) {
      pc.close();
      forget(inv);
      return setState(inv, 'failed', refused);
    }
    setState(inv, 'joined', 'joined.');
  }

  // One reply at a time: null (not this page's), or the text for the player.
  async function oneReply(text) {
    if (busy) return 'Another reply is being read. Try again in a moment.';
    busy = true;
    try {
      return await takeReply(text);
    } catch (e) {
      return message(e);
    } finally {
      busy = false;
    }
  }

  function showHost() {
    const p = openPanel('Host with a link');
    p.append(
      el('p', {
        text: `Friends join your game with an invite offer link. Create one for each friend: each works once, for ${minutes(INVITE_MS)}.`,
      }),
    );
    const choice = el('fieldset', {}, el('legend', { text: 'Connection server' }));
    const privacy = el('div');
    const show = () => privacy.replaceChildren(privacyText(server, 'Each player you invite'));
    for (const [key, s] of Object.entries(STUN_SERVERS)) {
      const id = `invite-server-${key}`;
      const radio = el('input', {
        type: 'radio',
        name: 'invite-server',
        id,
        value: key,
        checked: key === server,
        on: { change: () => ((server = key), show()) },
      });
      choice.append(el('div', { class: 'choice' }, radio, el('label', { htmlFor: id, text: s.name })));
    }
    show();
    said = el('p', { class: 'said', 'data-said': '' });
    list = list ?? el('ol', { class: 'list', reversed: true });
    const paste = el('input', { type: 'text', 'aria-label': "A player's reply link", placeholder: 'Paste a reply link' });
    const add = el('button', {
      type: 'button',
      text: 'Add',
      on: {
        click: async () => {
          said.textContent = 'Reading the reply...';
          const out = await oneReply(paste.value);
          said.textContent = out ?? 'This reply is not for an invite made on this page.';
          paste.value = '';
        },
      },
    });
    p.append(
      choice,
      privacy,
      el('button', { type: 'button', class: 'primary', text: 'Create Invite Offer', on: { click: makeInvite } }),
      list,
      el('h3', { text: 'A reply came back?' }),
      el('p', {
        text: 'Open the reply link in this browser and the game takes it. Or paste it here:',
      }),
      el('div', { class: 'row' }, paste, add),
      said,
      el('button', { type: 'button', class: 'close', text: 'Close', on: { click: closePanel } }),
    );
    p.querySelector('.primary').focus();
  }

  globalThis.torqueAsk = (what) => {
    if (what === 'host') showHost();
    else if (what === 'join') showJoinPaste();
  };

  // Replies handed over by a reply tab in this browser; answered only when the reply is for one of this page's invites
  // (or is no code at all).
  if (typeof BroadcastChannel === 'function') {
    const bc = new BroadcastChannel(channelName());
    bc.onmessage = async (e) => {
      const m = e.data;
      if (typeof m?.reply !== 'string' || typeof m.id !== 'string' || m.id.length > 32) return;
      const out = await oneReply(m.reply);
      if (out !== null) bc.postMessage({ answer: m.id, ok: out === TAKEN, text: out });
    };
  }

  return (h) => {
    host = h;
    if (open) showHost();
  };
}

// ---- a reply tab

// When the page was opened on a reply link (`fragment` from takeFragment): hands the reply to this browser's game tab and
// closes, or shows why not. Returns at once; the game does not start in this tab.
export async function replyTab(code) {
  const loading = document.getElementById('loading');
  loading?.remove();
  const p = openPanel('A reply link', backToMenu);
  const said = el('p', { 'data-said': '' });
  p.append(said);
  const root = document.documentElement;
  // A reply that was not taken: this tab can still go to the game's menu.
  const failed = (state) => {
    p.append(backButton());
    root.dataset.torqueReply = state;
  };
  try {
    await decodeCode(code, KIND_REPLY);
  } catch (e) {
    said.textContent = message(e);
    return failed('refused');
  }
  said.textContent = 'Handing the reply to your game...';
  const fallback = () => {
    said.textContent =
      'No game in this browser made this invite offer. Open the tab that made it, or paste this reply link into its ' +
      'Host with a link panel:';
    p.append(linkBox(`${pageLink()}#reply=${code}`, 'The reply link'));
    failed('unclaimed');
  };
  if (typeof BroadcastChannel !== 'function') return fallback();
  const bc = new BroadcastChannel(channelName());
  const id = newToken();
  const answer = await new Promise((ok) => {
    const t = setTimeout(() => ok(null), HANDOFF_MS);
    bc.onmessage = (e) => {
      if (e.data?.answer === id && typeof e.data.text === 'string') {
        clearTimeout(t);
        ok({ ok: e.data.ok === true, text: e.data.text });
      }
    };
    bc.postMessage({ reply: code, id });
  });
  bc.close();
  if (!answer) return fallback();
  said.textContent = answer.ok ? 'Your game took the reply. You can close this tab.' : answer.text;
  if (answer.ok) root.dataset.torqueReply = 'taken';
  else failed('refused');
  // A browser lets a page close its own tab only when the tab has no history before it (a link opened from a chat app).
  if (answer.ok && history.length <= 1) setTimeout(() => window.close(), 500);
}

// ---- the joiner

// The joiner's side of an invite (`code`, from the fragment): its panel shows at once; `attach(game)` once the module is
// loaded gives the game its link (the game joins once the channel opens, `startLinkClient`). Every state of the panel has
// Back to the menu, which opens the page again with no invite: its game played alone, at its menu. Esc does the same
// before Connect and once the join failed, never while the reply is being made or waits for the host.
export function joinInvite(code) {
  const root = document.documentElement;
  let state = 'reading';
  const p = openPanel('Join with Link', () => {
    if (state !== 'making' && state !== 'replied') backToMenu();
  });
  const said = el('p', { class: 'said', 'data-said': '' });
  const back = backButton();
  p.append(said, back);
  const show = (...nodes) => back.before(...nodes);
  let game = null;
  let channel = null;
  let opened = false;
  let ended = null;
  const mark = (next) => {
    state = next;
    root.dataset.torqueJoin = next;
  };
  const end = (why) => {
    if (ended) return;
    ended = why;
    if (state !== 'refused') mark('ended');
    game?.linkEnded(why);
  };
  // A join that cannot go on: why, and the way back. A reply link shown is taken away (its connection is closed).
  const fail = (text, next = 'ended') => {
    said.textContent = text;
    p.querySelector('.link')?.remove();
    if (!p.isConnected) {
      panel?.remove();
      document.body.append(p);
      panel = p;
    }
    if (next === 'refused') mark('refused');
    end(text);
    back.focus();
  };
  const refuse = (text) => fail(text, 'refused');

  // the menu's Play alone instead (an invited page's game, `torqueAsk('back')`)
  globalThis.torqueAsk = (what) => {
    if (what === 'back') backToMenu();
  };

  async function connect(data) {
    mark('making');
    said.textContent = 'Making your reply...';
    const lan = privateAllowed(data.server, location.search, true);
    // One certificate for every connection this join makes: each has the fingerprint the reply link names.
    const certificates = [await RTCPeerConnection.generateCertificate({ name: 'ECDSA', namedCurve: 'P-256' })];
    // This page's own mDNS names (every connection's): an invite that names them is not taken at them (a host could point
    // this browser at its own network address and a port of its choice). Addresses stay: two pages on one computer share
    // them.
    const own = new Set();
    const ownName = (address) => MDNS_NAME.test(address ?? '') && own.add(address);
    let pc = null;
    // The reply's ICE credentials, fingerprint and DTLS setup (`fromSdp`), once the first connection has made them.
    let details = null;
    let started = 0;
    // The states of this join's connections (`watchStates`), one history through every connection that took over.
    let seen = undefined;
    // Back to the menu ends this connection with the page.
    addEventListener('pagehide', () => pc?.close());
    // The channel did not open: the current connection's details are gathered, then it is closed.
    let failing = false;
    const noRelay = async () => {
      if (failing || opened || ended) return;
      failing = true;
      const last = pc;
      const found = await detailsOf(last, seen);
      // the channel opened while the details were gathered: it stays
      if (opened) return;
      last.close();
      if (ended) return;
      fail(NO_RELAY);
      said.after(detailsBox(found));
    };

    // A connection that answers the invite (as `details` say, once there are any) and is given the invite's addresses:
    // its own description, or null when the join ended or the connection was replaced meanwhile.
    async function answer() {
      const next = new RTCPeerConnection({ iceServers: iceServersFor(data.server), certificates });
      pc = next;
      seen = watchStates(next, seen);
      started = performance.now();
      next.ondatachannel = (e) => {
        const ch = e.channel;
        if (next !== pc) return;
        if (channel || ch.label !== 'game' || ch.ordered || ch.maxRetransmits !== 0) {
          next.close();
          return refuse('The host sent a connection this page does not take. Ask the host for a new invite offer link.');
        }
        channel = ch;
        ch.binaryType = 'arraybuffer';
        ch.onopen = () => {
          opened = true;
          mark('open');
          game?.linkOpened();
          closePanel();
        };
        // the host's messages within the same caps as a joiner's on the host: past them, the link ends
        ch.onmessage = meteredReceiver(
          (data) => game?.localDatagram(new Uint8Array(data)),
          () => {
            next.close();
            end('The host sent more than the game takes, so the connection was closed.');
          },
        );
        ch.onclose = () => (opened ? end('The host left the game.') : noRelay());
      };
      next.oniceconnectionstatechange = () => {
        if (next !== pc || next.iceConnectionState !== 'failed') return;
        if (opened) end('The host left the game.');
        else if (details) again();
      };
      next.onicecandidate = (e) => ownName(e.candidate?.candidate?.split(' ')[4]);
      await next.setRemoteDescription({ type: 'offer', sdp: toSdp(data) });
      const made = await next.createAnswer();
      await next.setLocalDescription(details ? { type: 'answer', sdp: withDetails(made.sdp, details) } : made);
      await gathered(next, GATHER_MS);
      if (ended || next !== pc) {
        next.close();
        return null;
      }
      const sdp = next.localDescription.sdp;
      if (details && !sameDetails(fromSdp(sdp), details)) throw new Error("the browser did not keep the reply link's details");
      for (const line of sdpLines(sdp)) ownName(line.split(' ')[4]);
      const lines = allowedLines(data, lan, own);
      if (!lines.length) {
        next.close();
        refuse('This invite offer has no address this page can reach. Ask the host for a new one.');
        return null;
      }
      for (const line of lines) await next.addIceCandidate({ candidate: line, sdpMid: '0' }).catch(() => {});
      return sdp;
    }

    // Before the channel opens, the browser may give up on the host's addresses while the reply link is on its way, and
    // then no longer answer the host's checks once the host takes it. Firefox does a few seconds after it answers when no
    // address of the host answers yet, as a Firefox host's mDNS names do not until it has taken the reply. A new
    // connection with the reply's credentials, certificate and setup takes over: once the host has the reply, the host
    // answers its checks (and takes its address as the reply's). At most one every RETRY_MS, until JOIN_MS ends the wait.
    function again() {
      if (failing) return;
      const old = pc;
      if (channel) {
        channel.onopen = channel.onclose = channel.onmessage = null;
        channel = null;
      }
      old.close();
      setTimeout(
        () => {
          if (opened || ended || failing || pc !== old) return;
          answer().catch(noRelay);
        },
        Math.max(0, started + RETRY_MS - performance.now()),
      );
    }

    const sdp = await answer();
    if (!sdp) return;
    details = fromSdp(sdp);
    const candidates = pickCandidates(sdpLines(sdp), (o) => addressAllowed(o, lan), { mdns: true });
    const reply = await encodeCode({ kind: KIND_REPLY, token: data.token, ...details, candidates });
    markUsed(data.token);
    said.textContent =
      'Send this reply link back to the host. When they open it in the browser that made the invite offer, you join ' +
      `their game. This page waits up to ${minutes(JOIN_MS)} for it.`;
    show(linkBox(`${pageLink()}#reply=${reply}`, 'Your reply link'));
    mark('replied');
    if (!opened && pc.iceConnectionState === 'failed') again();
    setTimeout(noRelay, JOIN_MS);
  }

  (async () => {
    let data;
    try {
      data = await decodeCode(code, KIND_JOIN);
      if (wasUsed(data.token)) throw new CodeError('This invite offer link was already used in this browser. Ask the host for a new one.');
    } catch (e) {
      return refuse(message(e));
    }
    const server = serverFor(data.server);
    const go = el('button', {
      type: 'button',
      class: 'primary',
      text: 'Connect',
      on: {
        click: () => {
          go.remove();
          connect(data).catch((e) => refuse(message(e)));
        },
      },
    });
    show(
      el('p', {
        text:
          (server.urls.length
            ? `This invite offer connects through ${server.name}'s public STUN server. Nothing is sent until you press Connect.`
            : 'This invite offer is for players on the same network as the host.') +
          ` It expires ${minutes(INVITE_MS)} after the host made it.`,
      }),
      privacyText(data.server, 'The host'),
      go,
    );
    said.textContent = '';
    mark('ready');
    go.focus();
  })();

  return {
    // Whether the invite was refused (a bad or used code): the page then starts no game.
    refused: () => state === 'refused',
    attach(module) {
      game = module;
      game.setLocalSender((bytes) => {
        if (channel?.readyState !== 'open' || channel.bufferedAmount >= MAX_BUFFERED) return;
        channel.send(bytes);
      });
      if (opened) game.linkOpened();
      if (ended) game.linkEnded(ended);
    },
  };
}

function usedTokens() {
  try {
    const list = JSON.parse(localStorage.getItem(USED_KEY) ?? '[]');
    return Array.isArray(list) ? list.filter((t) => typeof t === 'string') : [];
  } catch {
    return [];
  }
}
const wasUsed = (token) => usedTokens().includes(token);
function markUsed(token) {
  try {
    localStorage.setItem(USED_KEY, JSON.stringify([...usedTokens(), token].slice(-MAX_USED)));
  } catch {}
}
