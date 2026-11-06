// The connection codes' format (codes.js): `node --test web/codes.test.mjs`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { deflateRawSync } from 'node:zlib';
import * as c from './codes.js';
import { addressAllowed } from './host.js';

const fingerprint = Uint8Array.from({ length: 32 }, (_, i) => (i * 37 + 11) & 0xff);
const invite = (more = {}) => ({
  kind: c.KIND_JOIN,
  server: 'cloudflare',
  token: 'AbCdEfGh-_12',
  ufrag: 'x9Qa',
  pwd: 'h7Jk2Lm9Np4Qr6St8Uv0Wx3Y',
  fingerprint,
  setup: 'actpass',
  sctpPort: 5000,
  candidates: [
    { type: 'srflx', ip: '203.0.113.57', port: 54321 },
    { type: 'srflx', ip: '203.0.113.57', port: 61234 },
    { type: 'host', ip: '192.168.1.23', port: 50123 },
  ],
  ...more,
});
const reply = (more = {}) => {
  const { server, ...rest } = invite({ kind: c.KIND_REPLY, setup: 'active', ...more });
  return rest;
};

// A code made of `text` as it stands (no check of its lines), with `version` and `kind` bytes.
function rawCode(text, { version = c.CODE_VERSION, kind = c.KIND_JOIN, body = null } = {}) {
  const deflated = body ?? deflateRawSync(Buffer.from(text));
  const bytes = new Uint8Array(2 + deflated.length + 4);
  bytes[0] = version;
  bytes[1] = kind;
  bytes.set(deflated, 2);
  new DataView(bytes.buffer).setUint32(2 + deflated.length, c.crc32(bytes.subarray(0, 2 + deflated.length)));
  return c.toBase64url(bytes);
}

const refused = (promise, pattern) => assert.rejects(promise, (e) => e instanceof c.CodeError && pattern.test(e.message));

test('an invite and a reply read back as they were made', async () => {
  const made = invite();
  assert.deepEqual(await c.decodeCode(await c.encodeCode(made), c.KIND_JOIN), made);
  const back = reply({ candidates: [{ type: 'host', ip: '10.0.0.5', port: 60001 }] });
  assert.deepEqual(await c.decodeCode(await c.encodeCode(back), c.KIND_REPLY), back);
  const noCandidates = reply({ candidates: [], sctpPort: 5001 });
  assert.deepEqual(await c.decodeCode(await c.encodeCode(noCandidates), c.KIND_REPLY), noCandidates);
});

test('codes with two or three candidates stay under 300 characters', async () => {
  const sizes = [];
  for (const [name, made] of [
    ['invite, 3 candidates', invite()],
    ['invite, 2 candidates', invite({ server: 'google', candidates: invite().candidates.slice(1) })],
    ['reply, 3 candidates', reply()],
    // a browser whose ICE credentials are longer (8 and 32 characters)
    ['invite, 3 candidates, long credentials', invite({ ufrag: 'a1b2c3d4', pwd: '0123456789abcdef0123456789abcdef' })],
  ]) {
    const code = await c.encodeCode(made);
    sizes.push(`${name}: ${code.length}`);
    assert.ok(code.length < 300, `${name}: ${code.length} characters`);
  }
  console.log(`code sizes: ${sizes.join(', ')}`);
});

test('every server key reads back, and an unknown or malformed one is refused', async () => {
  for (const key of Object.keys(c.STUN_SERVERS)) {
    const back = await c.decodeCode(await c.encodeCode(invite({ server: key })), c.KIND_JOIN);
    assert.equal(back.server, key);
    assert.equal(c.serverFor(key), c.STUN_SERVERS[key]);
  }
  assert.deepEqual(c.iceServersFor('google'), [{ urls: ['stun:stun.l.google.com:19302', 'stun:stun1.l.google.com:19302'] }]);
  assert.deepEqual(c.iceServersFor('lan'), []);
  const lines = (server) => c.payloadText(invite()).replace('s cloudflare', `s ${server}`);
  await refused(c.decodeCode(rawCode(lines('evil')), c.KIND_JOIN), /does not know \("evil"\)/);
  // a name every object has is not a key
  await refused(c.decodeCode(rawCode(lines('constructor')), c.KIND_JOIN), /does not know \("constructor"\)/);
  for (const bad of ['Google', 'stun:evil.example', 'abcdefghijklmnopq', 'go0gle', '']) {
    await refused(c.decodeCode(rawCode(lines(bad)), c.KIND_JOIN), /not valid|not in its form/);
    assert.throws(() => c.serverFor(bad), /form this page does not accept/);
  }
  await assert.rejects(c.encodeCode(invite({ server: 'evil' })), c.CodeError);
});

test('a damaged, cut short or oversized code is refused with a message', async () => {
  const code = await c.encodeCode(invite());
  const flip = (i) => code.slice(0, i) + (code[i] === 'A' ? 'B' : 'A') + code.slice(i + 1);
  await refused(c.decodeCode(flip(20), c.KIND_JOIN), /check does not match/);
  await refused(c.decodeCode(flip(2), c.KIND_JOIN), /check does not match|different version/);
  for (const cut of [code.length - 1, code.length - 3, code.length - 8, 60, 5]) {
    await refused(c.decodeCode(code.slice(0, cut), c.KIND_JOIN), /cut short|check does not match/);
  }
  await refused(c.decodeCode('A'.repeat(c.MAX_CODE_CHARS + 1), c.KIND_JOIN), /too long/);
  await refused(c.decodeCode(`${code.slice(0, 40)}*${code.slice(41)}`, c.KIND_JOIN), /characters a code never has/);
  await refused(c.decodeCode('', c.KIND_JOIN), /No code/);
  await refused(c.decodeCode(`${code}=`, c.KIND_JOIN), /characters a code never has/);
});

test('a payload that inflates past the cap is refused, and so is one that is not a whole stream', async () => {
  // 4 KiB of one letter deflates to a few bytes
  const bomb = rawCode('a'.repeat(4096));
  assert.ok(bomb.length < 100);
  await refused(c.decodeCode(bomb, c.KIND_JOIN), /too large once unpacked/);
  const whole = deflateRawSync(Buffer.from(c.payloadText(invite())));
  await refused(c.decodeCode(rawCode('', { body: whole.subarray(0, whole.length - 6) }), c.KIND_JOIN), /damaged or cut short/);
  await refused(c.decodeCode(rawCode('', { body: Buffer.from([0xff, 0xfe, 0x00]) }), c.KIND_JOIN), /damaged or cut short/);
});

test('a code of the other kind or another version is refused', async () => {
  await refused(c.decodeCode(await c.encodeCode(invite()), c.KIND_REPLY), /invite link, not a reply/);
  await refused(c.decodeCode(await c.encodeCode(reply()), c.KIND_JOIN), /reply link, not an invite/);
  const text = c.payloadText(invite());
  await refused(c.decodeCode(rawCode(text, { version: 2 }), c.KIND_JOIN), /different version/);
  await refused(c.decodeCode(rawCode(text, { kind: 7 }), c.KIND_JOIN), /not an invite or a reply/);
});

test("a code's lines are read strictly", async () => {
  const text = c.payloadText(invite());
  const swap = (from, to) => c.decodeCode(rawCode(text.replace(from, to)), c.KIND_JOIN);
  const bad = /not valid/;
  await refused(swap('t AbCdEfGh-_12\n', ''), bad);
  await refused(swap('t AbCdEfGh-_12', 't AbCdEfGh-_12\nt AbCdEfGh-_12'), bad);
  await refused(swap('t AbCdEfGh-_12', 't AbCdEfGh'), bad);
  await refused(swap('u x9Qa', 'u x9'), bad);
  await refused(swap('d actpass', 'd active'), bad);
  await refused(swap('d actpass', 'd actpass\nx 5000'), bad);
  await refused(swap('d actpass', 'd actpass\nx 70000'), bad);
  await refused(swap('203.0.113.57 54321', '203.000.113.57 54321'), bad);
  await refused(swap('203.0.113.57 54321', '203.0.113.256 54321'), bad);
  await refused(swap('203.0.113.57 54321', 'host.local 54321'), bad);
  await refused(swap('203.0.113.57 54321', '203.0.113.57 0'), bad);
  await refused(swap('203.0.113.57 54321', '203.0.113.57 054321'), bad);
  await refused(swap('203.0.113.57 54321', '203.0.113.57 99999'), bad);
  await refused(swap('c srflx 203.0.113.57 54321', 'c relay 203.0.113.57 54321'), bad);
  await refused(swap('c srflx 203.0.113.57 61234', 'c srflx 203.0.113.57 54321'), /twice/);
  await refused(swap('d actpass', 'd actpass\nz 1'), bad);
  await refused(c.decodeCode(rawCode(`${text}\n`), c.KIND_JOIN), bad);
  await refused(c.decodeCode(rawCode(text.replaceAll('\n', '\r\n')), c.KIND_JOIN), bad);
  await refused(c.decodeCode(rawCode(`${text}é`), c.KIND_JOIN), bad);
  const many = Array.from({ length: c.MAX_CODE_CANDIDATES + 1 }, (_, i) => `c host 10.0.0.${i + 1} 5000`).join('\n');
  await refused(c.decodeCode(rawCode(`${text}\n${many}`), c.KIND_JOIN), /more than 16/);
  const back = reply();
  await refused(c.decodeCode(rawCode(c.payloadText(back).replace('d active', 'd actpass'), { kind: c.KIND_REPLY }), c.KIND_REPLY), bad);
});

test('a pasted link or code gives its code, and a fragment its kind', () => {
  assert.equal(c.codeFromPaste('https://example.org/fps/#join=AbC-_9', c.KIND_JOIN), 'AbC-_9');
  assert.equal(c.codeFromPaste('  AbC-_9  ', c.KIND_REPLY), 'AbC-_9');
  assert.equal(c.codeFromPaste('https://example.org/fps/#reply=AbC', c.KIND_JOIN), null);
  assert.equal(c.codeFromPaste('https://example.org/fps/#join=AbC&via=x', c.KIND_JOIN), null);
  assert.equal(c.codeFromPaste('not a code!', c.KIND_JOIN), null);
  assert.deepEqual(c.codeFromFragment('#reply=xyz'), { kind: 'reply', code: 'xyz' });
  assert.equal(c.codeFromFragment('#other=xyz'), null);
  assert.equal(c.codeFromFragment(''), null);
});

test('base64url is read one way only', () => {
  for (let n = 0; n < 10; n++) {
    const bytes = Uint8Array.from({ length: n }, (_, i) => (i * 91 + n) & 0xff);
    assert.deepEqual(c.fromBase64url(c.toBase64url(bytes)), bytes);
  }
  assert.equal(c.fromBase64url('AB'), null); // its last bits are not zero
  assert.equal(c.fromBase64url('A'), null);
  assert.equal(c.fromBase64url('AA=='), null);
  assert.equal(c.newToken().length, c.TOKEN_CHARS);
});

const SDP = [
  'v=0',
  'o=- 4611731400430051336 2 IN IP4 127.0.0.1',
  's=-',
  't=0 0',
  'a=group:BUNDLE 0',
  'm=application 9 UDP/DTLS/SCTP webrtc-datachannel',
  'c=IN IP4 0.0.0.0',
  'a=candidate:1 1 udp 2122260223 192.168.1.23 50123 typ host generation 0 network-id 1',
  'a=candidate:2 1 tcp 1518280447 192.168.1.23 9 typ host tcptype active generation 0',
  'a=candidate:3 1 udp 2122260223 0f6c-uuid.local 50124 typ host generation 0',
  'a=candidate:4 1 udp 1686052607 93.184.216.34 54321 typ srflx raddr 0.0.0.0 rport 0 generation 0',
  'a=candidate:5 1 udp 1686052607 93.184.216.34 54321 typ srflx raddr 0.0.0.0 rport 0 generation 0',
  'a=candidate:6 1 udp 2122260223 127.0.0.1 50125 typ host generation 0',
  'a=candidate:7 2 udp 2122260222 192.168.1.23 50126 typ host generation 0',
  'a=ice-ufrag:x9Qa',
  'a=ice-pwd:h7Jk2Lm9Np4Qr6St8Uv0Wx3Y',
  'a=ice-options:trickle',
  `a=fingerprint:sha-256 ${[...fingerprint].map((b) => b.toString(16).toUpperCase().padStart(2, '0')).join(':')}`,
  'a=setup:actpass',
  'a=mid:0',
  'a=sctp-port:5000',
  'a=max-message-size:262144',
  '',
].join('\r\n');

test("a page's own description gives a code's parts, and a code's parts a description", async () => {
  const parts = c.fromSdp(SDP);
  assert.deepEqual(parts, { ufrag: 'x9Qa', pwd: 'h7Jk2Lm9Np4Qr6St8Uv0Wx3Y', fingerprint, setup: 'actpass', sctpPort: 5000 });
  const lines = SDP.split('\r\n').filter((l) => l.startsWith('a=candidate:'));
  // public only (93.184.216.x: an example.com range): the srflx once
  assert.deepEqual(c.pickCandidates(lines, (o) => addressAllowed(o, false)), [{ type: 'srflx', ip: '93.184.216.34', port: 54321 }]);
  // with private ones: the srflx first; never TCP, `.local` names, loopback or component 2
  assert.deepEqual(c.pickCandidates(lines, (o) => addressAllowed(o, true)), [
    { type: 'srflx', ip: '93.184.216.34', port: 54321 },
    { type: 'host', ip: '192.168.1.23', port: 50123 },
  ]);
  const many = Array.from({ length: 9 }, (_, i) => `a=candidate:${i} 1 udp 1 10.0.0.${i + 1} 5000 typ host`);
  assert.equal(c.pickCandidates(many, () => true).length, c.PICKED_CANDIDATES);
  const rebuilt = c.toSdp({ ...parts });
  assert.match(rebuilt, /^m=application 9 UDP\/DTLS\/SCTP webrtc-datachannel$/m);
  assert.equal(rebuilt.match(/^m=/gm).length, 1);
  assert.deepEqual(c.fromSdp(rebuilt), parts);
  assert.equal(
    c.candidateLine({ type: 'srflx', ip: '203.0.113.57', port: 54321 }, 1),
    'candidate:2 1 udp 1694498559 203.0.113.57 54321 typ srflx raddr 0.0.0.0 rport 0',
  );
  assert.throws(() => c.fromSdp(SDP.replace(/a=fingerprint:.*\r\n/, '')), /without ICE or DTLS/);
});
