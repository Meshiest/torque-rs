// Connection codes: what two players trade by hand (a link in a chat) to open a WebRTC data channel between their pages
// with no server of ours. The host's invite (`#join=<code>`) carries its offer; the joiner's reply (`#reply=<code>`) its
// answer. No page needed: the tests run it in Node (`node --test web/codes.test.mjs`).
//
// A code is base64url (no padding) of: a version byte (CODE_VERSION), a kind byte (KIND_JOIN or KIND_REPLY), the payload
// deflated (`deflate-raw`), and a CRC-32 of everything before it (big-endian). The payload is the SDP cut to what a data
// channel needs, one ASCII line each, in this order:
//   s <server>                the STUN server's key in STUN_SERVERS (invites only)
//   t <token>                 the invite's token, 12 base64url characters (a reply repeats its invite's)
//   u <ice-ufrag>             4 to 32 ice-chars
//   p <ice-pwd>               22 to 64 ice-chars
//   f <fingerprint>           the DTLS certificate's SHA-256, 43 base64url characters
//   d <setup>                 actpass (invites), active or passive (replies)
//   x <sctp-port>             only when not 5000
//   c <type> <ip> <port>      0 to MAX_CODE_CANDIDATES: host, srflx or prflx, a canonical IPv4 address, a plain port
// A code is untrusted input: its length is capped before it is decoded, its inflated size while it inflates, and every
// line must match its pattern exactly (no line twice, none missing, none extra). A code never names a server: the server
// is a key into this page's own fixed list, and an unknown key is refused.

export const CODE_VERSION = 1;
export const KIND_JOIN = 1;
export const KIND_REPLY = 2;
// The longest code read (characters, before decoding).
export const MAX_CODE_CHARS = 1024;
// The most bytes a payload may inflate to.
export const MAX_TEXT_BYTES = 2048;
// The most candidates a code may carry (a code made here carries at most PICKED_CANDIDATES).
export const MAX_CODE_CANDIDATES = 16;
export const PICKED_CANDIDATES = 4;
export const TOKEN_CHARS = 12;
// The version and kind bytes, the CRC, and at least one deflated byte.
const MIN_BYTES = 2 + 1 + 4;

// The STUN servers a page may use, by the key a code carries. Never extended from a code.
export const STUN_SERVERS = Object.freeze({
  cloudflare: Object.freeze({
    name: 'Cloudflare',
    urls: Object.freeze(['stun:stun.cloudflare.com:3478']),
    privacy: 'https://www.cloudflare.com/privacypolicy/',
  }),
  google: Object.freeze({
    name: 'Google',
    urls: Object.freeze(['stun:stun.l.google.com:19302', 'stun:stun1.l.google.com:19302']),
    privacy: 'https://policies.google.com/privacy',
  }),
  lan: Object.freeze({ name: 'Same network only', urls: Object.freeze([]), privacy: null }),
});
export const DEFAULT_SERVER = 'cloudflare';

// A refused code, with a message for the player.
export class CodeError extends Error {
  constructor(message) {
    super(message);
    this.name = 'CodeError';
  }
}

// The STUN server under `key`, or a CodeError: lowercase a to z only, at most 16 characters, and a key of STUN_SERVERS.
export function serverFor(key) {
  if (typeof key !== 'string' || !/^[a-z]{1,16}$/.test(key)) {
    throw new CodeError('This invite names its connection server in a form this page does not accept.');
  }
  if (!Object.hasOwn(STUN_SERVERS, key)) {
    throw new CodeError(`This invite asks for a connection server this page does not know ("${key}").`);
  }
  return STUN_SERVERS[key];
}

// The `RTCPeerConnection` ICE servers for `key` (a fresh copy).
export function iceServersFor(key) {
  const { urls } = serverFor(key);
  return urls.length ? [{ urls: [...urls] }] : [];
}

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

export function crc32(bytes) {
  let c = 0xffffffff;
  for (const b of bytes) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

const B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';

export function toBase64url(bytes) {
  let out = '';
  for (let i = 0; i < bytes.length; i += 3) {
    const n = (bytes[i] << 16) | ((bytes[i + 1] ?? 0) << 8) | (bytes[i + 2] ?? 0);
    const chars = Math.min(4, Math.ceil(((bytes.length - i) * 8) / 6));
    for (let j = 0; j < chars; j++) out += B64[(n >> (18 - 6 * j)) & 63];
  }
  return out;
}

// The bytes of base64url `text`, or null (a character outside the alphabet, or a length no encoding has).
export function fromBase64url(text) {
  if (!/^[A-Za-z0-9_-]*$/.test(text) || text.length % 4 === 1) return null;
  const out = new Uint8Array(Math.floor((text.length * 6) / 8));
  let bits = 0;
  let acc = 0;
  let o = 0;
  for (const ch of text) {
    acc = (acc << 6) | B64.indexOf(ch);
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out[o++] = (acc >> bits) & 0xff;
    }
  }
  // the bits left over must be zero (one text per byte string)
  if (acc & ((1 << bits) - 1)) return null;
  return out;
}

async function pipe(bytes, stream) {
  const writer = stream.writable.getWriter();
  writer.write(bytes).catch(() => {});
  writer.close().catch(() => {});
  return stream.readable.getReader();
}

async function deflate(bytes) {
  const reader = await pipe(bytes, new CompressionStream('deflate-raw'));
  const parts = [];
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    parts.push(value);
  }
  return concat(parts);
}

// The inflated bytes, or a CodeError: the input is not a whole deflate stream, or it inflates past MAX_TEXT_BYTES (it stops
// there, never inflating the rest).
async function inflate(bytes) {
  const reader = await pipe(bytes, new DecompressionStream('deflate-raw'));
  const parts = [];
  let size = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.length;
      if (size > MAX_TEXT_BYTES) {
        reader.cancel().catch(() => {});
        throw new CodeError('This code is too large once unpacked.');
      }
      parts.push(value);
    }
  } catch (e) {
    if (e instanceof CodeError) throw e;
    throw new CodeError('This code is damaged or cut short. Copy the whole link again.');
  }
  return concat(parts);
}

function concat(parts) {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

const ICE_CHAR = '[A-Za-z0-9+/]';
const LINES = {
  s: /^[a-z]{1,16}$/,
  t: new RegExp(`^[A-Za-z0-9_-]{${TOKEN_CHARS}}$`),
  u: new RegExp(`^${ICE_CHAR}{4,32}$`),
  p: new RegExp(`^${ICE_CHAR}{22,64}$`),
  f: /^[A-Za-z0-9_-]{43}$/,
  d: /^(actpass|active|passive)$/,
  x: /^[1-9]\d{0,4}$/,
  c: /^(host|srflx|prflx) (\S+) ([1-9]\d{0,4})$/,
};

// An IPv4 address in canonical dotted form as its four numbers, or null (as host.js's `ipv4`).
function ipv4(text) {
  const m = /^(0|[1-9]\d{0,2})\.(0|[1-9]\d{0,2})\.(0|[1-9]\d{0,2})\.(0|[1-9]\d{0,2})$/.exec(text ?? '');
  if (!m) return null;
  const octets = m.slice(1).map(Number);
  return octets.every((o) => o <= 255) ? octets : null;
}

// What a code says, as an object: { kind, server (invites), token, ufrag, pwd, fingerprint (32 bytes), setup, sctpPort,
// candidates: [{ type, ip, port }] }.
export function payloadText(data) {
  const lines = [];
  if (data.kind === KIND_JOIN) lines.push(`s ${data.server}`);
  lines.push(`t ${data.token}`, `u ${data.ufrag}`, `p ${data.pwd}`, `f ${toBase64url(data.fingerprint)}`, `d ${data.setup}`);
  if (data.sctpPort !== 5000) lines.push(`x ${data.sctpPort}`);
  for (const c of data.candidates) lines.push(`c ${c.type} ${c.ip} ${c.port}`);
  return lines.join('\n');
}

// The payload's lines read strictly, or a CodeError naming the first problem.
export function parsePayload(text, kind) {
  const bad = (why) => new CodeError(`This code's contents are not valid: ${why}.`);
  if (!/^[\x20-\x7e\n]*$/.test(text)) throw bad('it has characters a code never has');
  const lines = text.split('\n');
  const order = kind === KIND_JOIN ? ['s', 't', 'u', 'p', 'f', 'd'] : ['t', 'u', 'p', 'f', 'd'];
  const data = { kind, sctpPort: 5000, candidates: [] };
  let i = 0;
  const field = (key) => {
    const line = lines[i];
    if (line === undefined || !line.startsWith(`${key} `)) throw bad(`the "${key}" line is missing`);
    const value = line.slice(2);
    const m = LINES[key].exec(value);
    if (!m) throw bad(`the "${key}" line is not in its form`);
    i++;
    return m;
  };
  for (const key of order) {
    const value = field(key)[0];
    if (key === 's') data.server = value;
    else if (key === 't') data.token = value;
    else if (key === 'u') data.ufrag = value;
    else if (key === 'p') data.pwd = value;
    else if (key === 'f') data.fingerprint = fromBase64url(value);
    else if (key === 'd') data.setup = value;
  }
  if (!data.fingerprint || data.fingerprint.length !== 32) throw bad('the fingerprint is not 32 bytes');
  if (kind === KIND_JOIN ? data.setup !== 'actpass' : data.setup === 'actpass') throw bad('the DTLS setup is not one this side takes');
  if (lines[i]?.startsWith('x ')) {
    const port = Number(field('x')[0]);
    if (port > 65535 || port === 5000) throw bad('the data channel port is not in its form');
    data.sctpPort = port;
  }
  const seen = new Set();
  while (i < lines.length) {
    if (data.candidates.length >= MAX_CODE_CANDIDATES) throw bad(`it has more than ${MAX_CODE_CANDIDATES} addresses`);
    const [, type, ip, portText] = field('c');
    const port = Number(portText);
    const octets = ipv4(ip);
    if (!octets || octets.join('.') !== ip || port > 65535) throw bad('an address is not in its form');
    if (seen.has(`${ip}:${port}`)) throw bad('an address is there twice');
    seen.add(`${ip}:${port}`);
    data.candidates.push({ type, ip, port });
  }
  if (kind === KIND_JOIN) serverFor(data.server);
  return data;
}

// The code for `data` (see payloadText).
export async function encodeCode(data) {
  const text = payloadText(data);
  // what is made must read back
  parsePayload(text, data.kind);
  const body = await deflate(new TextEncoder().encode(text));
  const bytes = new Uint8Array(2 + body.length + 4);
  bytes[0] = CODE_VERSION;
  bytes[1] = data.kind;
  bytes.set(body, 2);
  new DataView(bytes.buffer).setUint32(2 + body.length, crc32(bytes.subarray(0, 2 + body.length)));
  return toBase64url(bytes);
}

// What code `text` says (as payloadText's object), if it is a code of `kind`; else a CodeError for the player.
export async function decodeCode(text, kind) {
  if (typeof text !== 'string' || text.trim() === '') throw new CodeError('No code was given.');
  const code = text.trim();
  if (code.length > MAX_CODE_CHARS) throw new CodeError('This code is too long to be one of ours.');
  const bytes = fromBase64url(code);
  if (!bytes) throw new CodeError('This code has characters a code never has, or is cut short. Copy the whole link again.');
  if (bytes.length < MIN_BYTES) throw new CodeError('This code is cut short. Copy the whole link again.');
  const end = bytes.length - 4;
  if (new DataView(bytes.buffer).getUint32(end) !== crc32(bytes.subarray(0, end))) {
    throw new CodeError('This code is damaged or cut short (its check does not match). Copy the whole link again.');
  }
  if (bytes[0] !== CODE_VERSION) throw new CodeError('This code was made by a different version of this page.');
  if (bytes[1] !== kind) {
    if (bytes[1] === KIND_JOIN) throw new CodeError('This is an invite link, not a reply.');
    if (bytes[1] === KIND_REPLY) throw new CodeError('This is a reply link, not an invite.');
    throw new CodeError('This code is not an invite or a reply.');
  }
  const inflated = await inflate(bytes.subarray(2, end));
  let payload;
  try {
    payload = new TextDecoder('utf-8', { fatal: true }).decode(inflated);
  } catch {
    throw new CodeError("This code's contents are not valid: it is not text.");
  }
  return parsePayload(payload, kind);
}

// The code in a pasted link or code (`...#join=<code>`, `...#reply=<code>`, or the code alone) of `kind`, as text; null when
// there is none of that kind. Nothing else of the link is used.
export function codeFromPaste(text, kind) {
  const word = kind === KIND_JOIN ? 'join' : 'reply';
  const t = String(text ?? '').trim();
  if (t.length > MAX_CODE_CHARS + 4096) return null;
  const hash = t.indexOf('#');
  if (hash < 0) return /^[A-Za-z0-9_-]+$/.test(t) ? t : null;
  const m = new RegExp(`^${word}=([^&]*)$`).exec(t.slice(hash + 1));
  return m ? m[1] : null;
}

// The code a page's fragment (`location.hash`) carries: { kind: 'join' | 'reply', code } or null.
export function codeFromFragment(hash) {
  const m = /^#(join|reply)=([^&]*)$/.exec(String(hash ?? ''));
  return m ? { kind: m[1], code: m[2] } : null;
}

// A fresh invite token.
export function newToken() {
  const bytes = new Uint8Array(9);
  crypto.getRandomValues(bytes);
  return toBase64url(bytes);
}

// The parts of a page's own SDP (offer or answer) a code carries, but the candidates: { ufrag, pwd, fingerprint, setup,
// sctpPort }, or an Error.
export function fromSdp(sdp) {
  const value = (name) => {
    const line = sdp.split(/\r?\n/).find((l) => l.startsWith(`a=${name}:`));
    return line?.slice(name.length + 3).trim();
  };
  const fp = /^sha-256 ((?:[0-9A-Fa-f]{2}:){31}[0-9A-Fa-f]{2})$/.exec(value('fingerprint') ?? '');
  const data = {
    ufrag: value('ice-ufrag'),
    pwd: value('ice-pwd'),
    fingerprint: fp ? Uint8Array.from(fp[1].split(':'), (h) => parseInt(h, 16)) : null,
    setup: value('setup'),
    sctpPort: Number(value('sctp-port') ?? 5000),
  };
  if (!data.ufrag || !data.pwd || !data.fingerprint || !data.setup) throw new Error('the browser made a description without ICE or DTLS details');
  return data;
}

// The candidates a code carries from candidate lines (with or without `a=`): component 1, UDP, a canonical IPv4 address
// `allowed(octets)` takes (no `.local` names), host, srflx or prflx; each address once, srflx first, at most
// PICKED_CANDIDATES.
export function pickCandidates(lines, allowed) {
  const picked = [];
  const seen = new Set();
  for (const raw of lines) {
    const words = String(raw).trim().replace(/^a=/, '').split(' ');
    if (words.length < 8 || !words[0].startsWith('candidate:') || words[1] !== '1' || words[6] !== 'typ') continue;
    const [, , transport, , ip, portText, , type] = words;
    const octets = ipv4(ip);
    const port = /^[1-9]\d{0,4}$/.test(portText) ? Number(portText) : 0;
    if (transport.toLowerCase() !== 'udp' || !octets || octets.join('.') !== ip || port < 1 || port > 65535) continue;
    if (!['host', 'srflx', 'prflx'].includes(type) || !allowed(octets) || seen.has(`${ip}:${port}`)) continue;
    seen.add(`${ip}:${port}`);
    picked.push({ type, ip, port });
  }
  const rank = { srflx: 0, prflx: 1, host: 2 };
  return picked.sort((a, b) => rank[a.type] - rank[b.type]).slice(0, PICKED_CANDIDATES);
}

// The other side's description, rebuilt from a code's parts: one data channel's m-line and nothing else.
export function toSdp(data) {
  const hex = [...data.fingerprint].map((b) => b.toString(16).toUpperCase().padStart(2, '0')).join(':');
  return [
    'v=0',
    'o=- 1 1 IN IP4 127.0.0.1',
    's=-',
    't=0 0',
    'a=group:BUNDLE 0',
    'm=application 9 UDP/DTLS/SCTP webrtc-datachannel',
    'c=IN IP4 0.0.0.0',
    `a=ice-ufrag:${data.ufrag}`,
    `a=ice-pwd:${data.pwd}`,
    `a=fingerprint:sha-256 ${hex}`,
    `a=setup:${data.setup}`,
    'a=mid:0',
    `a=sctp-port:${data.sctpPort}`,
    'a=max-message-size:262144',
    '',
  ].join('\r\n');
}

// A code's candidate as a candidate line (its priority from its type and place; the line goes through host.js's
// `candidateAllowed` before the browser sees it).
export function candidateLine(c, index) {
  const typePref = { host: 126, prflx: 110, srflx: 100 }[c.type];
  const priority = typePref * 2 ** 24 + (65535 - index) * 2 ** 8 + 255;
  const related = c.type === 'host' ? '' : ' raddr 0.0.0.0 rport 0';
  return `candidate:${index + 1} 1 udp ${priority} ${c.ip} ${c.port} typ ${c.type}${related}`;
}
