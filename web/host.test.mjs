// The browser host's rules for what joiners and the room service send, without a page: `node --test web/host.test.mjs`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { MAX_CANDIDATES, candidateAllowed, dataOnly, iceServersFrom, peerNumber, splitCandidates } from './host.js';

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
