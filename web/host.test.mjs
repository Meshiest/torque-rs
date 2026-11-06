// The browser host's rules for what joiners and the room service send, without a page: `node --test web/host.test.mjs`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  MAX_CANDIDATES,
  candidateAllowed,
  channelLive,
  dataOnly,
  iceServersFrom,
  meteredReceiver,
  nextLinkId,
  peerNumber,
  splitCandidates,
} from './host.js';

const line = (address, transport = 'udp', port = 40000) =>
  `candidate:1 1 ${transport} 2122260223 ${address} ${port} typ host generation 0`;

test('a joiner names only addresses the host may check', () => {
  assert.deepEqual(candidateAllowed(line('93.184.216.40'), false), { line: line('93.184.216.40'), ip: '93.184.216.40' });
  assert.ok(candidateAllowed(`a=${line('93.184.216.40')}`, false), 'with its a=');
  for (const refused of [
    '127.0.0.1',
    '0.0.0.0',
    '0.1.2.3',
    '255.255.255.255',
    '224.0.0.251',
    '239.255.255.250',
    '240.0.0.1',
    '169.254.169.254',
    '192.0.2.1',
    '198.51.100.1',
    '203.0.113.1',
    '198.18.0.1',
    '100.64.0.1',
    '192.0.0.8',
    '256.1.1.1',
    '2001:4860::1',
    'abcd-1234.local',
  ]) {
    assert.equal(candidateAllowed(line(refused), true), null, refused);
  }
  for (const priv of ['10.0.0.2', '172.16.0.2', '192.168.1.2']) {
    assert.equal(candidateAllowed(line(priv), false), null, `${priv} without LAN play`);
    assert.ok(candidateAllowed(line(priv), true), `${priv} with LAN play`);
  }
  assert.equal(candidateAllowed(line('93.184.216.40', 'tcp'), false), null, 'TCP');
  assert.equal(candidateAllowed(line('93.184.216.40', 'udp', 0), false), null, 'no port');
  assert.equal(candidateAllowed(line('93.184.216.40', 'udp', 70000), false), null, 'a port past 65535');
  assert.equal(candidateAllowed(line('93.184.216.40'), false, new Set(['93.184.216.40'])), null, "the host's own");
  // Only canonical forms: a leading zero could be read as another address (octal) past the checks.
  for (const odd of ['010.0.0.1', '127.000.000.001', '0127.0.0.1', '93.184.216.040', '00.1.2.3', '93.184.216.40.', ' 93.184.216.40']) {
    assert.equal(candidateAllowed(line(odd), true), null, odd);
  }
  assert.equal(candidateAllowed(line('93.184.216.40', 'udp', '040000'), false), null, 'a port with a leading zero');
  assert.equal(candidateAllowed(line('8.8.8.8'), false).ip, '8.8.8.8');
  assert.equal(candidateAllowed('candidate:garbage', false), null);
  assert.equal(candidateAllowed(`${line('93.184.216.40')} ${'x'.repeat(600)}`, false), null, 'too long');
  assert.equal(candidateAllowed(42, false), null);
});

test("an offer's candidates are taken out, a few at most", () => {
  const sdp = ['v=0', 'm=application 9 UDP/DTLS/SCTP webrtc-datachannel', `a=${line('1.2.3.4')}`, 'a=mid:0', ''].join('\r\n');
  const [kept, lines] = splitCandidates(sdp);
  assert.equal(kept, ['v=0', 'm=application 9 UDP/DTLS/SCTP webrtc-datachannel', 'a=mid:0', ''].join('\r\n'));
  assert.deepEqual(lines, [line('1.2.3.4')]);
  assert.equal(splitCandidates(`a=${line('1.2.3.4')}\n`.repeat(500))[1].length, MAX_CANDIDATES * 4);
  assert.ok(dataOnly(sdp));
  assert.ok(!dataOnly(`${sdp}m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n`));
  assert.ok(!dataOnly('v=0'));
});

test("the room's peer numbers and ICE servers are read strictly", () => {
  assert.equal(peerNumber('7'), 7);
  assert.equal(peerNumber('4294967295'), 4294967295);
  for (const bad of ['0', '07', '-1', '1.5', '', '4294967296', '99999999999', 'x', undefined]) {
    assert.equal(peerNumber(bad), null, String(bad));
  }
  const servers = iceServersFrom(
    JSON.stringify([
      { urls: 'stun:stun.example.org:3478' },
      { urls: ['turn:turn.example.org:3478?transport=udp'], username: '1700000000:ab', credential: 'c2VjcmV0' },
      { urls: 'turn:turn.example.org' },
      { urls: 'http://example.org' },
      { urls: 42 },
      null,
    ]),
  );
  assert.deepEqual(servers, [
    { urls: ['stun:stun.example.org:3478'] },
    { urls: ['turn:turn.example.org:3478?transport=udp'], username: '1700000000:ab', credential: 'c2VjcmV0' },
  ]);
  assert.deepEqual(iceServersFrom('not json'), []);
  assert.deepEqual(iceServersFrom('{"urls":"stun:x"}'), []);
  assert.equal(iceServersFrom(JSON.stringify(Array(50).fill({ urls: 'stun:x' }))).length, 8);
});

test("a link's receiver ends the link on a flood, an oversize message or text, and delivers nothing after", () => {
  let t = 5000;
  const run = (messages) => {
    const got = [];
    let ends = 0;
    const take = meteredReceiver((d) => got.push(d), () => ends++, () => t);
    for (const data of messages) take({ data });
    return { got: got.length, ends };
  };
  // 512 a second pass; the 513th ends the link, and the rest are dropped
  assert.deepEqual(run(Array.from({ length: 600 }, () => new ArrayBuffer(100))), { got: 512, ends: 1 });
  assert.deepEqual(run([new ArrayBuffer(1500), new ArrayBuffer(1501), new ArrayBuffer(10)]), { got: 1, ends: 1 });
  assert.deepEqual(run([new ArrayBuffer(10), 'text', new ArrayBuffer(10)]), { got: 1, ends: 1 });
  // a new second starts a new count
  const got = [];
  const take = meteredReceiver((d) => got.push(d), () => assert.fail('ended'), () => t);
  for (let s = 0; s < 3; s++, t += 1000) for (let i = 0; i < 512; i++) take({ data: new ArrayBuffer(1) });
  assert.equal(got.length, 1536);
});

test("a browser's mDNS name is taken only for invite links, and only a lowercase version 4 UUID", () => {
  const name = '4f9c2a1e-7b3d-4c8e-9a10-2b6e8d4f1c3a.local';
  const mdns = { mdns: true };
  assert.deepEqual(candidateAllowed(line(name), true, new Set(), mdns), { line: line(name), ip: name });
  // an invite on a STUN server (no private addresses) takes it too: two tabs of one browser, or two computers on one network
  assert.deepEqual(candidateAllowed(line(name), false, new Set(), mdns), { line: line(name), ip: name });
  assert.equal(candidateAllowed(line(name), true, new Set()), null, 'not without mdns (rooms)');
  assert.equal(candidateAllowed(line(name), false, new Set()), null);
  assert.equal(candidateAllowed(line(name), true, new Set([name]), mdns), null, "the host's own name");
  assert.equal(candidateAllowed(line(name, 'tcp'), true, new Set(), mdns), null);
  assert.equal(candidateAllowed(line(name).replace('typ host', 'typ srflx'), true, new Set(), mdns), null);
  assert.equal(candidateAllowed(`${line(name)} raddr ${name} rport 0`, true, new Set(), mdns), null, 'a second name');
  for (const nearMiss of [
    'evil.local',
    '4F9C2A1E-7B3D-4C8E-9A10-2B6E8D4F1C3A.local',
    'x.4f9c2a1e-7b3d-4c8e-9a10-2b6e8d4f1c3a.local',
    '4f9c2a1e-7b3d-1c8e-9a10-2b6e8d4f1c3a.local',
    '4f9c2a1e-7b3d-4c8e-7a10-2b6e8d4f1c3a.local',
    '4f9c2a1e7b3d4c8e9a102b6e8d4f1c3a.local',
    '4f9c2a1e-7b3d-4c8e-9a10-2b6e8d4f1c3a.local.example.com',
    '4f9c2a1e-7b3d-4c8e-9a10-2b6e8d4f1c3a.LOCAL',
    '4f9c2a1e-7b3d-4c8e-9a10-2b6e8d4f1c3a.local.',
    '4f9c2a1e-7b3d-4c8e-9a10-2b6e8d4f1c3a',
  ]) {
    assert.equal(candidateAllowed(line(nearMiss), true, new Set(), mdns), null, nearMiss);
  }
});

test('an invited joiner takes a link number not just freed, never 0', () => {
  const taken = new Set([1, 2]);
  let [id, next] = nextLinkId(taken, 1);
  assert.deepEqual([id, next], [3, 4]);
  taken.add(id);
  // link 3 ends: the next joiner gets 4, not 3 again
  taken.delete(3);
  [id, next] = nextLinkId(taken, next);
  assert.equal(id, 4);
  // past the last number it starts again at 1, skipping those taken
  assert.deepEqual(nextLinkId(new Set([0xffffffff, 1]), 0xffffffff), [2, 3]);
  assert.deepEqual(nextLinkId(new Set(), 0), [1, 2]);
});

test('a channel that closed while it waited is not handed to the game', () => {
  assert.equal(channelLive({ readyState: 'open' }), true);
  assert.equal(channelLive({ readyState: 'connecting' }), true);
  assert.equal(channelLive({ readyState: 'closing' }), false);
  assert.equal(channelLive({ readyState: 'closed' }), false);
  assert.equal(channelLive(null), false);
});
