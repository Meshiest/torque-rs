// Games joined by invite links, on a page with no server of ours (a solo page: `build.mjs --solo`). The host's page runs
// the game's server in its worker (host.js); the host makes one invite link per player (`<page>#join=<code>`, codes.js),
// each for one peer connection of its own, and sends it in a chat. The player's page opens it, makes its answer, and shows
// a reply code (the bare text of codes.js `encodeCode`, not a link) to hand back. The host pastes it into the game's
// Host with a link panel, under "A reply came back?" (an old reply link, `<page>#reply=<code>`, is read the same way; a page
// opened on one only shows its code). The data channel then opens straight between the two browsers, and the joiner's
// channel goes to the worker's relay like any other link.
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
// A connection that does not open shows its details under the message, to copy into a report: which side wrote them,
// the browser's family, the invite's server, how the connection's states went, counts of its candidates and candidate
// pairs by type and state, and what became of the other side's candidates (what its code carried, what the filter kept
// and why it dropped the rest, what the browser refused) (`connectionDetails`, from the connection's stats just before
// it is closed). They never hold an address, a port, an mDNS name, a ufrag or a code.
//
// Caps: at most MAX_INVITES invites wait at once, each for INVITE_MS; a reply has CONNECT_MS for its channel to open; one
// reply is read at a time; a code is refused whole when it breaks any of codes.js's caps; an invite's reply is taken once.
//
// Nothing here opens the page again: the page's game runs from its first load on (`soloPage`). An invite link opens the
// page playing alone with the join's panel over its menu; the game switches to the host's once the channel opens
// (`switchTo('link')`), and back to playing alone when the player goes back or leaves that game (`switchTo('solo')`).
import {
  CodeError,
  DEFAULT_SERVER,
  KIND_JOIN,
  KIND_REPLY,
  MAX_CODE_CHARS,
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
const MAX_BUFFERED = 256 * 1024;
const USED_KEY = 'torque-invites-used';
const MAX_USED = 32;
const TAKEN = 'The reply is in: connecting to the player.';
const TRUST = 'Only send this link to people you trust. It contains your IP address.';
const REPLY_TRUST = 'Only send this to the host. It contains your IP address.';
const NO_RELAY =
  'Could not connect. Check that the host pasted your reply code into the page that made the invite offer, then try a new ' +
  "invite offer on the other connection server. Some networks can't connect without a relay server, which this page " +
  'does not use; players on one network can pick Same network only.';

// Whether private addresses may be used: the same-network server, or on the host's own page `?lan=1` when the page's code
// allows it (`pageLan`, `soloPage`'s and `hostInvites`' `lan`): a link alone never turns it on. A joining page ignores
// `?lan`: its address came from the host, so only the invite's server decides.
export function privateAllowed(server, search, joining, pageLan = false) {
  return server === 'lan' || (!joining && pageLan && new URLSearchParams(search).has('lan'));
}
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
let backdrop = null;
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
  panelUp();
  return panel;
}
// While a panel is open the mouse is the panel's: the game takes no look or wheel from it (`data-torque-panel`, read by the
// game's window), and a backdrop over the game takes the clicks beside the panel, so none locks the pointer.
function panelUp() {
  backdrop ??= el('div', { id: 'invites-backdrop' });
  panel.before(backdrop);
  document.documentElement.dataset.torquePanel = '1';
}
function closePanel() {
  panel?.remove();
  panel = null;
  backdrop?.remove();
  delete document.documentElement.dataset.torquePanel;
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
      `${who === 'The host' ? 'your reply code' : 'the link'}. `,
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

// A reply code with its copy button and the warning that only the host gets it.
function codeBox(code, label) {
  const { row, said } = copyRow(code, label, 'data-code', 'code');
  return el('div', { class: 'link' }, row, el('p', { class: 'warn', text: REPLY_TRUST }), said);
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

const GATHERING_STATES = ['new', 'gathering', 'complete'];
const SIDES = ['host', 'joiner'];
// Why the candidate filter (`filterCandidates`) left one of the other side's out, in the order the details list them.
const DROPPED = ['own', 'private', 'reserved', 'malformed'];

// One line of a failed connection's details, from the browser's user agent, the states `seen` (`watchStates`), the
// connection's stats (a list of `getStats` entries) and what the page knows of it (`facts`): which side wrote the line
// (`host` or `joiner`), the invite's server key, the joiner's attempts (a Firefox joiner makes a new connection while
// it waits), the browser's own states, ICE gathering's state at the failure, the local candidates by type (host ones as
// mDNS names or addresses), the other side's candidates as its code carried them (`code`), how many the filter kept and
// why it dropped the others (`kept`, `dropped`), how many the browser refused to add (`addFailed`), the remote
// candidates the browser holds (a remote one with no address is unresolved: Chrome reports an mDNS name it could not
// resolve so) and the candidate pairs by state. So "remote none" tells apart: the code carried none, the filter dropped
// them, the browser refused them, or (all kept and added) it could not resolve their mDNS names. Counts and fixed words
// only: nothing of an address, port, name, ufrag or code, whatever the stats or facts hold.
export function connectionDetails(userAgent, seen, stats, facts = {}) {
  const word = (v, known) => (known.includes(v) ? v : 'other');
  const number = (v) => (Number.isSafeInteger(v) && v >= 0 ? v : 0);
  const count = (map, key) => map.set(key, (map.get(key) ?? 0) + 1);
  const newSide = () => ({ types: new Map(), host: new Map() });
  const sides = { local: newSide(), remote: newSide(), code: newSide() };
  const pairs = new Map();
  let unresolved = 0;
  const hostKind = (address) => (address.toLowerCase().endsWith('.local') ? 'mDNS' : 'address');
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
      count(side.host, hostKind(address));
    }
  }
  for (const c of Array.isArray(facts.code) ? facts.code : []) {
    const type = word(c?.type, CANDIDATE_TYPES);
    count(sides.code.types, type);
    if (type === 'host') count(sides.code.host, hostKind(String(c?.ip ?? '')));
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
  const dropped = DROPPED.filter((k) => number(facts.dropped?.[k]) > 0).map((k) => `${number(facts.dropped[k])} ${k}`);
  const side = word(facts.side, SIDES);
  return [
    side,
    browserFamily(userAgent),
    `server ${word(facts.server, Object.keys(STUN_SERVERS))}`,
    ...(side === 'joiner' ? [`attempt ${number(facts.attempts)}`] : []),
    `ice ${history(seen?.ice)}`,
    `connection ${history(seen?.peer)}`,
    `gathering ${facts.gathering === undefined ? 'none' : word(facts.gathering, GATHERING_STATES)}`,
    `local ${candidates(sides.local)}`,
    `code ${candidates(sides.code)}`,
    `kept ${number(facts.kept)}, dropped ${dropped.length ? dropped.join(', ') : 'none'}`,
    `adds failed ${number(facts.addFailed)}`,
    `remote ${candidates(sides.remote)}${unresolved ? `, ${unresolved} unresolved` : ''}`,
    `pairs ${list(pairs, PAIR_STATES, (k) => `${pairs.get(k)} ${k}`)}`,
  ].join('; ');
}

// The details of `pc` (with the page's `facts` of it, `connectionDetails`), gathered before it is closed (a closed
// connection has no stats).
async function detailsOf(pc, seen, facts) {
  let stats = [];
  try {
    stats = [...(await pc.getStats()).values()];
  } catch {}
  return connectionDetails(navigator.userAgent, seen, stats, { ...facts, gathering: pc.iceGatheringState });
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

// The code's candidates as lines the browser may take (each through `candidateAllowed`, mDNS names included), with how
// many were kept and why the others were dropped: `own` (one of this page's own addresses or names), `private` (a
// private address, allowed only on the same-network server), `reserved` (an address no invite takes) or `malformed`.
export function filterCandidates(data, lan, own) {
  const lines = [];
  const dropped = { own: 0, private: 0, reserved: 0, malformed: 0 };
  data.candidates.forEach((c, i) => {
    const line = candidateLine(c, i);
    const taken = candidateAllowed(line, lan, own, { mdns: true });
    if (taken) lines.push(taken.line);
    else if (own.has(c.ip)) dropped.own++;
    else if (candidateAllowed(line, true, new Set(), { mdns: true })) dropped.private++;
    else if (ipv4(c.ip) && !addressAllowed(ipv4(c.ip), true)) dropped.reserved++;
    else dropped.malformed++;
  });
  return { lines, kept: lines.length, dropped };
}

// Adds each line to `pc`, counting in `facts.addFailed` the ones the browser refused.
async function addLines(pc, lines, facts) {
  for (const line of lines) {
    await pc.addIceCandidate({ candidate: line, sdpMid: '0' }).catch(() => facts.addFailed++);
  }
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

// ---- the joiner's paste field

// A field to paste an invite offer link into and its Join button: `go(code)` with the link's code once it reads as an
// invite offer (codes.js's caps whole); else the line under it says why not.
function joinPasteRow(go) {
  const paste = el('input', { type: 'text', 'aria-label': 'An invite offer link', placeholder: 'Paste an invite offer link' });
  const said = el('p', { class: 'said', 'data-said': '' });
  const join = el('button', {
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
        go(code);
      },
    },
  });
  paste.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') join.click();
  });
  return { row: el('div', { class: 'row' }, paste, join), said, paste };
}

// The menu's Join with Link: the paste field; `go(code)` starts the join here.
function showJoinPaste(go) {
  const p = openPanel('Join with Link');
  const { row, said, paste } = joinPasteRow(go);
  p.append(
    el('p', { text: 'Paste the invite offer link a friend sent you. Your own game here ends when you join theirs.' }),
    row,
    said,
    el('button', { type: 'button', class: 'close', text: 'Close', on: { click: closePanel } }),
  );
  paste.focus();
}

// ---- the host

// An invite's states once it is over: its line then has a Dismiss button that takes it off the list.
const OVER = ['failed', 'expired', 'left', 'closed'];

// The host's invites (one for the page): `hosting(h)` is host.js's
// `onHost` (`h` null once that server stopped), `show()` opens the Host with a link panel, `away(true)` closes every
// invite still open and the panel and lets no one in (the page is in another player's game) until `away(false)`.
// `open`: the panel opens once the server runs. `lan`: whether a `?lan=1` link may let invites use private addresses on
// other servers than the same-network one (the page's choice, see `privateAllowed`).
export function hostInvites({ open = false, lan = false } = {}) {
  const lanFor = (key) => privateAllowed(key, location.search, false, lan);
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
  // the Host with a link panel, while it is open
  let hostPanel = null;
  // the page is in another player's game: no invite is made or taken, and no one comes in
  let gone = false;
  // the invites' list is hidden (Hide invites: no link on screen while streaming), for the page's session; then the
  // count line says what is on it
  let hidden = false;
  let toggle = null;
  let countLine = null;
  const AWAY = "You are in another player's game. Go back to your own to let friends in.";
  const root = document.documentElement;

  const pending = () => [...invites.values()].filter((i) => i.state === 'waiting' || i.state === 'making').length;

  // every invite on the list, for the page's `data-torque-invites` (each one's state, oldest first)
  const made = [];
  const marked = () => {
    root.dataset.torqueInvites = made.map((i) => i.state).join(' ');
    counted();
  };
  // The list shown or hidden as `hidden` says, with its toggle's text and the count line.
  function shown() {
    if (!list || !toggle) return;
    list.hidden = hidden;
    toggle.textContent = hidden ? 'Show invites' : 'Hide invites';
    toggle.setAttribute('aria-expanded', String(!hidden));
    counted();
  }
  // "3 invites, 1 waiting, 1 joined", while the list is hidden.
  function counted() {
    if (!countLine) return;
    countLine.hidden = !hidden;
    if (!hidden) return (countLine.textContent = '');
    const n = made.length;
    const of = (states) => made.filter((i) => states.includes(i.state)).length;
    const parts = [
      [of(['making', 'waiting']), 'waiting'],
      [of(['connecting']), 'connecting'],
      [of(['joined']), 'joined'],
    ].filter(([k]) => k > 0);
    const words = [`${n} invite${n === 1 ? '' : 's'}`, ...parts.map(([k, w]) => `${k} ${w}`)];
    countLine.textContent = `${words.join(', ')}.`;
  }
  // An invite's link is shown only while it waits for its reply: once the reply is in, or the invite expired or failed,
  // its link box goes (its status line stays, and a failed connection's details, until it is dismissed).
  function setState(inv, state, text) {
    inv.state = state;
    inv.status.textContent = `Invite ${inv.n}: ${text}`;
    if (state !== 'waiting') inv.box?.remove();
    if (OVER.includes(state) && !inv.dismiss) {
      inv.dismiss = el('button', { type: 'button', class: 'dismiss', text: 'Dismiss', on: { click: () => dismiss(inv) } });
      inv.item.append(inv.dismiss);
    }
    marked();
  }

  // An invite that is over, off the list for good: it counts for nothing more, and a reply for it is still told it is
  // no longer open (its token stays closed).
  function dismiss(inv) {
    forget(inv);
    inv.item.remove();
    const at = made.indexOf(inv);
    if (at >= 0) made.splice(at, 1);
    marked();
    panel?.querySelector('.primary')?.focus();
  }

  function forget(inv) {
    clearTimeout(inv.timer);
    invites.delete(inv.token);
    closed.add(inv.token);
  }

  async function makeInvite() {
    if (gone) {
      said.textContent = AWAY;
      return;
    }
    if (pending() >= MAX_INVITES) {
      said.textContent = `${MAX_INVITES} invite offers are waiting already. Wait for a reply or for one to expire.`;
      return;
    }
    const key = server;
    const inv = { n: ++count, token: newToken(), server: key, state: 'making', own: new Set(), status: el('p') };
    // what this page knows of the connection, for its details if it fails (`connectionDetails`)
    inv.facts = { side: 'host', server: key, code: [], kept: 0, dropped: {}, addFailed: 0 };
    invites.set(inv.token, inv);
    if (made.length < 64) made.push(inv);
    const item = el('li', {}, inv.status);
    inv.item = item;
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

  // Takes a reply (a code, or an old reply link, pasted): null when no invite of this page is its, else the text for
  // the player (TAKEN once the reply is in; the invite's line then says how the connection went).
  async function takeReply(text) {
    const code = codeFromPaste(text, KIND_REPLY);
    if (code === null) return 'This is not a reply code.';
    const data = await decodeCode(code, KIND_REPLY);
    if (used.has(data.token)) return 'This reply code was already used. To let this player in again, create a new invite offer.';
    if (closed.has(data.token)) return 'That invite offer is no longer open. Create a new one.';
    const inv = invites.get(data.token);
    if (!inv || inv.state !== 'waiting') return null;
    if (gone) return AWAY;
    used.add(data.token);
    clearTimeout(inv.timer);
    setState(inv, 'connecting', 'connecting...');
    const { pc } = inv;
    try {
      await pc.setRemoteDescription({ type: 'answer', sdp: toSdp(data) });
      const { lines, kept, dropped } = filterCandidates(data, lanFor(inv.server), inv.own);
      Object.assign(inv.facts, { code: data.candidates, kept, dropped });
      await addLines(pc, lines, inv.facts);
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
    const opened = await waitOpen(ch, CONNECT_MS);
    // closed meanwhile (`closeAll`)
    if (!invites.has(inv.token)) return pc.close();
    if (!opened) {
      const details = await detailsOf(pc, inv.seen, inv.facts);
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
    const left = () => {
      forget(inv);
      setState(inv, 'left', 'left the game.');
    };
    const refused = gone ? AWAY : host ? host.adopt(pc, ch, ip, left) : 'The game is not running.';
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
    hostPanel = p;
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
    list = list ?? el('ol', { class: 'list', id: 'invites-list', reversed: true });
    toggle = el('button', {
      type: 'button',
      class: 'toggle',
      'aria-controls': 'invites-list',
      on: { click: () => ((hidden = !hidden), shown()) },
    });
    countLine = el('p', { class: 'count', 'data-count': '' });
    shown();
    const paste = el('input', {
      type: 'text',
      'aria-label': "Paste the player's reply code",
      placeholder: "Paste the player's reply code",
    });
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
      toggle,
      countLine,
      list,
      el('h3', { text: 'A reply came back?' }),
      el('p', {
        text: 'When a player sends back the reply code from their page, paste it here:',
      }),
      el('div', { class: 'row' }, paste, add),
      said,
      el('button', { type: 'button', class: 'close', text: 'Close', on: { click: closePanel } }),
    );
    p.querySelector('.primary').focus();
  }

  // Every invite still open closed: the page is joining another game. (The players in this page's game are ended by its
  // host, host.js.)
  function closeAll() {
    for (const inv of [...invites.values()]) {
      if (!['making', 'waiting', 'connecting'].includes(inv.state)) continue;
      inv.pc?.close();
      forget(inv);
      setState(inv, 'closed', 'closed: you joined another game.');
    }
  }

  return {
    hosting(h) {
      host = h;
      if (h && open) {
        open = false;
        showHost();
      }
    },
    show: showHost,
    away(on) {
      gone = on;
      if (!on) return;
      closeAll();
      if (panel && panel === hostPanel) closePanel();
    },
  };
}

// ---- a reply that reached the wrong page

// When the page was opened on a reply link (`code` from takeFragment): a reply goes to the host's panel, not to a page of
// its own, so the panel says so and shows the code to copy. The game does not start in this tab unless the player goes on
// to its menu: `back()` starts it here.
export function replyNote(code, { back = null } = {}) {
  const loading = document.getElementById('loading');
  if (loading) loading.style.display = 'none';
  let gone = false;
  const toMenu = () => {
    if (gone) return;
    gone = true;
    closePanel();
    if (loading) loading.style.display = '';
    back?.();
  };
  const p = openPanel('A reply for a host', toMenu);
  p.append(el('p', { 'data-said': '', text: "This is a reply for a host. Paste it into the host's panel instead." }));
  if (code.length <= MAX_CODE_CHARS && /^[A-Za-z0-9_-]+$/.test(code)) p.append(codeBox(code, 'The reply code'));
  p.append(el('button', { type: 'button', class: 'close', text: 'Back to the menu', on: { click: toMenu } }));
  p.querySelector('.close').focus();
  document.documentElement.dataset.torqueReply = 'shown';
}

// ---- the joiner

// The joiner's side of an invite (`code`, from the fragment or pasted): its panel shows at once; `attach(game)` once the
// module is loaded gives the game the link (`setLinkSender`, `linkDatagram`). When the channel opens, `opening()` switches
// the page's game to it (`switchTo('link')`) and the game joins (`linkOpened`). Every state of the panel has Back to the
// menu (`back()`: the page's own game, which ran under the panel all along); Esc does the same before Connect and once the
// join failed, never while the reply is being made or waits for the host. A join that failed or was refused also offers
// a field for a new invite offer link (`again(code)`). `dispose()` ends the join for good: its connection closed, its
// timers cleared, its panel gone, and nothing it does after reaches the page or the game.
export function joinInvite(code, { back = closePanel, again = null, opening = null } = {}) {
  const root = document.documentElement;
  let state = 'reading';
  let disposed = false;
  const timers = new Set();
  // `fn` after `ms`, unless the join was disposed by then
  const later = (fn, ms) => {
    const t = setTimeout(() => {
      timers.delete(t);
      if (!disposed) fn();
    }, ms);
    timers.add(t);
  };
  const p = openPanel('Join with Link', () => {
    if (state !== 'making' && state !== 'replied') back();
  });
  const said = el('p', { class: 'said', 'data-said': '' });
  // how the connection goes, once the reply code is made: in the panel, over the game's own status line under it
  const status = el('p', { class: 'status', role: 'status', 'data-status': '' });
  const setStatus = (text) => !disposed && (status.textContent = text);
  const backButton = el('button', { type: 'button', class: 'close', text: 'Back to the menu', on: { click: () => back() } });
  p.append(said, status, backButton);
  const show = (...nodes) => backButton.before(...nodes);
  let game = null;
  let pc = null;
  let channel = null;
  let opened = false;
  let ended = null;
  // Back to the menu, or the page closed: this join's connection ends with it.
  const hide = () => pc?.close();
  addEventListener('pagehide', hide);
  const mark = (next) => {
    if (disposed) return;
    state = next;
    root.dataset.torqueJoin = next;
  };
  const end = (why) => {
    if (ended || disposed) return;
    ended = why;
    if (state !== 'refused') mark('ended');
    if (opened) game?.linkEnded(why);
  };
  // The game takes the open channel: the page's game switches to it, and joins.
  const linked = () => {
    opening?.();
    game.linkOpened();
  };
  // A field for a new invite offer link, once.
  let offered = false;
  const offerNew = () => {
    if (offered || !again) return;
    offered = true;
    const { row, said: pasteSaid } = joinPasteRow(again);
    show(el('p', { text: 'Got a new invite offer link? Paste it here to join with it:' }), row, pasteSaid);
  };
  // A join that cannot go on: why, the way back and a field for a new link. A reply code shown is taken away (its
  // connection is closed).
  const fail = (text, next = 'ended') => {
    if (disposed) return;
    said.textContent = text;
    status.textContent = '';
    p.querySelector('.link')?.remove();
    p.querySelector('.reply-heading')?.remove();
    if (!p.isConnected) {
      panel?.remove();
      document.body.append(p);
      panel = p;
      panelUp();
    }
    if (next === 'refused') mark('refused');
    end(text);
    offerNew();
    backButton.focus();
  };
  const refuse = (text) => fail(text, 'refused');

  async function connect(data) {
    mark('making');
    said.textContent = 'Making your reply...';
    const lan = privateAllowed(data.server, location.search, true);
    // One certificate for every connection this join makes: each has the fingerprint the reply code names.
    const certificates = [await RTCPeerConnection.generateCertificate({ name: 'ECDSA', namedCurve: 'P-256' })];
    if (disposed) return;
    // This page's own mDNS names (every connection's): an invite that names them is not taken at them (a host could point
    // this browser at its own network address and a port of its choice). Addresses stay: two pages on one computer share
    // them.
    const own = new Set();
    const ownName = (address) => MDNS_NAME.test(address ?? '') && own.add(address);
    // The reply's ICE credentials, fingerprint and DTLS setup (`fromSdp`), once the first connection has made them.
    let details = null;
    let started = 0;
    // The states of this join's connections (`watchStates`), one history through every connection that took over.
    let seen = undefined;
    // What this page knows of its connections, for their details (`connectionDetails`): the filter's counts are the
    // latest connection's, the attempts and refused adds every connection's.
    const facts = { side: 'joiner', server: data.server, attempts: 0, code: data.candidates, kept: 0, dropped: {} };
    facts.addFailed = 0;
    // The channel did not open: the current connection's details are gathered, then it is closed.
    let failing = false;
    const noRelay = async () => {
      if (failing || opened || ended || disposed) return;
      failing = true;
      const last = pc;
      const found = await detailsOf(last, seen, facts);
      // the channel opened while the details were gathered: it stays
      if (opened) return;
      last.close();
      if (ended || disposed) return;
      fail(NO_RELAY);
      said.after(detailsBox(found));
    };

    // A connection that answers the invite (as `details` say, once there are any) and is given the invite's addresses:
    // its own description, or null when the join ended or the connection was replaced meanwhile.
    async function answer() {
      const next = new RTCPeerConnection({ iceServers: iceServersFor(data.server), certificates });
      pc = next;
      facts.attempts++;
      seen = watchStates(next, seen);
      started = performance.now();
      next.ondatachannel = (e) => {
        const ch = e.channel;
        if (next !== pc || disposed) return;
        if (channel || ch.label !== 'game' || ch.ordered || ch.maxRetransmits !== 0) {
          next.close();
          return refuse('The host sent a connection this page does not take. Ask the host for a new invite offer link.');
        }
        channel = ch;
        ch.binaryType = 'arraybuffer';
        ch.onopen = () => {
          if (disposed) return;
          opened = true;
          setStatus('Connected');
          mark('open');
          if (game) linked();
          if (panel === p) closePanel();
          else p.remove();
        };
        // the host's messages within the same caps as a joiner's on the host: past them, the link ends
        ch.onmessage = meteredReceiver(
          (data) => disposed || game?.linkDatagram(new Uint8Array(data)),
          () => {
            next.close();
            end('The host sent more than the game takes, so the connection was closed.');
          },
        );
        ch.onclose = () => (opened ? end('The host left the game.') : noRelay());
      };
      next.oniceconnectionstatechange = () => {
        if (next !== pc || disposed) return;
        const ice = next.iceConnectionState;
        // the host answered this page's checks: it has the reply
        if ((ice === 'connected' || ice === 'completed') && !opened && !ended) setStatus('Connecting...');
        if (ice !== 'failed') return;
        if (opened) end('The host left the game.');
        else if (details) again();
      };
      next.onicecandidate = (e) => ownName(e.candidate?.candidate?.split(' ')[4]);
      await next.setRemoteDescription({ type: 'offer', sdp: toSdp(data) });
      const made = await next.createAnswer();
      await next.setLocalDescription(details ? { type: 'answer', sdp: withDetails(made.sdp, details) } : made);
      await gathered(next, GATHER_MS);
      if (disposed || ended || next !== pc) {
        next.close();
        return null;
      }
      const sdp = next.localDescription.sdp;
      if (details && !sameDetails(fromSdp(sdp), details)) throw new Error("the browser did not keep the reply code's details");
      for (const line of sdpLines(sdp)) ownName(line.split(' ')[4]);
      const { lines, kept, dropped } = filterCandidates(data, lan, own);
      Object.assign(facts, { kept, dropped });
      if (!lines.length) {
        next.close();
        refuse('This invite offer has no address this page can reach. Ask the host for a new one.');
        return null;
      }
      await addLines(next, lines, facts);
      return sdp;
    }

    // Before the channel opens, the browser may give up on the host's addresses while the reply code is on its way, and
    // then no longer answer the host's checks once the host takes it. Firefox does a few seconds after it answers when no
    // address of the host answers yet, as a Firefox host's mDNS names do not until it has taken the reply. A new
    // connection with the reply's credentials, certificate and setup takes over: once the host has the reply, the host
    // answers its checks (and takes its address as the reply's). At most one every RETRY_MS, until JOIN_MS ends the wait.
    function again() {
      if (failing || disposed) return;
      setStatus('Waiting for the host...');
      const old = pc;
      if (channel) {
        channel.onopen = channel.onclose = channel.onmessage = null;
        channel = null;
      }
      old.close();
      later(
        () => {
          if (opened || ended || failing || pc !== old) return;
          answer().catch(noRelay);
        },
        Math.max(0, started + RETRY_MS - performance.now()),
      );
    }

    const sdp = await answer();
    if (!sdp || disposed) return;
    details = fromSdp(sdp);
    const candidates = pickCandidates(sdpLines(sdp), (o) => addressAllowed(o, lan), { mdns: true });
    const reply = await encodeCode({ kind: KIND_REPLY, token: data.token, ...details, candidates });
    if (disposed) return;
    markUsed(data.token);
    said.textContent =
      'The host pastes it into their Host with a link panel, under "A reply came back?", and you join their game. ' +
      `This page waits up to ${minutes(JOIN_MS)} for it.`;
    said.before(el('h3', { class: 'reply-heading', text: 'Send this code back to the host' }));
    said.after(codeBox(reply, 'Your reply code'));
    const reached = ['connected', 'completed'].includes(pc.iceConnectionState);
    if (!opened) setStatus(reached ? 'Connecting...' : 'Waiting for the host...');
    mark('replied');
    if (!opened && pc.iceConnectionState === 'failed') again();
    later(noRelay, JOIN_MS);
  }

  (async () => {
    let data;
    try {
      data = await decodeCode(code, KIND_JOIN);
      if (wasUsed(data.token)) throw new CodeError('This invite offer link was already used in this browser. Ask the host for a new one.');
    } catch (e) {
      return refuse(message(e));
    }
    if (disposed) return;
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
    // Whether the invite was refused (a bad or used code).
    refused: () => state === 'refused',
    attach(module) {
      game = module;
      if (disposed) return;
      game.setLinkSender((bytes) => {
        if (disposed || channel?.readyState !== 'open' || channel.bufferedAmount >= MAX_BUFFERED) return;
        channel.send(bytes);
      });
      if (opened) linked();
      if (opened && ended) game.linkEnded(ended);
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      for (const t of timers) clearTimeout(t);
      timers.clear();
      removeEventListener('pagehide', hide);
      if (channel) channel.onopen = channel.onclose = channel.onmessage = null;
      pc?.close();
      if (panel === p) closePanel();
      else p.remove();
      delete root.dataset.torqueJoin;
    },
  };
}

// ---- a page played alone

// The invite panels of a page played alone (`build.mjs --solo`) and its game's switches in place (the game's `switchTo`):
// played alone (`solo`), an invite link's game (`link`) and hosting with links (`host`). The page's `torqueAsk` takes the
// game's menus' asks: `host` (the Host with a link panel), `join` (the paste field), `back` (the page's own game again).
// `join(code)` starts a join here (an invite link the page was opened on, or one pasted); `attach(game, { startHost })`
// once the module is loaded; `onHost` is host.js's `onHost` and `hosted(handle)` takes `hostGame`'s handle, for a game
// whose own server runs in the page's worker from the start (the arena: joining another game ends its players, and the
// server runs on). `startHost(onHost)`, for a game played alone in the page itself (the vehicle demo), starts that server
// when the page first hosts and returns its handle (joining another game stops it). `lan` is `hostInvites`' `lan`.
export function soloPage({ lan = false } = {}) {
  let game = null;
  let startHost = null;
  let join = null;
  let invites = null;
  let hosted = null;
  let starting = null;
  let mode = 'solo';
  const book = () => (invites ??= hostInvites({ lan }));

  function joinWith(code) {
    if (mode === 'link') back();
    join?.dispose();
    join = joinInvite(code, { back, again: joinWith, opening });
    if (game) join.attach(game);
  }

  // The join's channel opened: the page leaves its own game (and any it hosts) for the host's.
  function opening() {
    mode = 'link';
    invites?.away(true);
    if (startHost) {
      hosted?.stop();
      hosted = null;
      invites?.hosting(null);
    } else {
      hosted?.endPeers();
    }
    game.switchTo('link');
  }

  // Back to the menu (the join's panel) and Play alone instead (the game's menu, or its invite link's game left): the
  // join ends, and the page's own game is played alone again.
  function back() {
    join?.dispose();
    join = null;
    if (mode !== 'link') return;
    mode = 'solo';
    invites?.away(false);
    game?.switchTo('solo');
  }

  async function host() {
    if (mode === 'link' || !game) return;
    const page = book();
    if (startHost) {
      try {
        const handle = hosted ?? (await (starting ??= startHost(page.hosting)));
        // the join's channel opened while the server started: this page is in the other game now
        if (mode === 'link') {
          if (handle !== hosted) handle.stop();
          page.hosting(null);
          return;
        }
        hosted = handle;
      } catch (e) {
        const p = openPanel('Host with a link');
        p.append(
          el('p', { class: 'said', 'data-said': '', text: `The game could not be hosted: ${String(e?.message ?? e)}` }),
          el('button', { type: 'button', class: 'close', text: 'Close', on: { click: closePanel } }),
        );
        return;
      } finally {
        starting = null;
      }
      mode = 'host';
      game.switchTo('host');
    }
    page.show();
  }

  globalThis.torqueAsk = (what) => {
    if (what === 'host') host();
    else if (what === 'join') showJoinPaste(joinWith);
    else if (what === 'back') back();
  };

  return {
    join: joinWith,
    attach(module, { startHost: start = null } = {}) {
      game = module;
      startHost = start;
      join?.attach(module);
    },
    onHost: (h) => book().hosting(h),
    hosted(handle) {
      hosted = handle;
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
