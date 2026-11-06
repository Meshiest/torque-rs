// The invite panels' rules that need no page: `node --test web/invites.test.mjs`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { browserFamily, connectionDetails, filterCandidates, minutes, privateAllowed } from './invites.js';

test("a joining page ignores ?lan: only the invite's server allows private addresses", () => {
  assert.equal(privateAllowed('lan', '', true), true);
  assert.equal(privateAllowed('cloudflare', '?lan=1', true), false, 'a link the host sent with ?lan=1');
  assert.equal(privateAllowed('google', '?nolock=1&lan', true), false);
  // the host's own page may choose it
  assert.equal(privateAllowed('cloudflare', '?lan=1', false), true);
  assert.equal(privateAllowed('cloudflare', '', false), false);
  assert.equal(privateAllowed('lan', '', false), true);
});

// User agents with their product tokens left out (the scripts here name no browser product: web/guard-scan.mjs); the page
// goes by the engine tokens.
const AGENTS = {
  Blink: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Safari/537.36',
  Edge: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Safari/537.36 Edg/141.0.0.0',
  Gecko: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:145.0) Gecko/20100101',
  WebKit:
    'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.4 Safari/605.1.15',
};

test("the browser's family comes from its user agent's engine tokens", () => {
  for (const [family, ua] of Object.entries(AGENTS)) assert.equal(browserFamily(ua), family);
  // every browser on an iPhone runs on WebKit
  const iphone = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko)';
  assert.equal(browserFamily(`${iphone} CriOS/141.0 Mobile/15E148 Safari/604.1`), 'WebKit');
  assert.equal(browserFamily(`${iphone} EdgiOS/141.0 Mobile/15E148 Safari/605.1.15`), 'WebKit');
  assert.equal(browserFamily('Mozilla/5.0 (Linux; Android 15) AppleWebKit/537.36 (KHTML, like Gecko) EdgA/141.0'), 'Edge');
  assert.equal(browserFamily('curl/8.0'), 'other');
  assert.equal(browserFamily(undefined), 'other');
});

// A failed connection's stats as a Blink browser reports them: its own host candidate by mDNS name and its public one, the other
// side's mDNS name unresolved (no address), an address and a public one; pairs failed, waiting and frozen.
const STATS = [
  { type: 'local-candidate', candidateType: 'host', address: '0b1d2c3e-4f50-4a6b-8c7d-9e0f1a2b3c4d.local', port: 54321 },
  { type: 'local-candidate', candidateType: 'srflx', address: '203.0.113.7', port: 61000, usernameFragment: 'AbCd' },
  { type: 'remote-candidate', candidateType: 'host', address: '', port: 50000 },
  { type: 'remote-candidate', candidateType: 'host', address: '192.168.1.20', port: 50001 },
  { type: 'remote-candidate', candidateType: 'srflx', ip: '198.51.100.9', port: 50002 },
  { type: 'candidate-pair', state: 'failed', localCandidateId: 'L1', remoteCandidateId: 'R1' },
  { type: 'candidate-pair', state: 'failed' },
  { type: 'candidate-pair', state: 'waiting' },
  { type: 'candidate-pair', state: 'frozen' },
  { type: 'transport', iceRole: 'controlling', dtlsState: 'new' },
  { type: 'certificate', fingerprint: 'AA:BB' },
];

// The other side's candidates as its code carried them, and what the page's filter and the browser made of them.
const NAME = '0b1d2c3e-4f50-4a6b-8c7d-9e0f1a2b3c4d.local';
const CODE = [
  { type: 'srflx', ip: '203.0.113.7', port: 61000 },
  { type: 'host', ip: NAME, port: 54321 },
  { type: 'host', ip: '192.168.1.20', port: 50001 },
];
const FACTS = {
  side: 'joiner',
  server: 'cloudflare',
  attempts: 3,
  gathering: 'complete',
  code: CODE,
  kept: 2,
  dropped: { own: 0, private: 1, reserved: 0, malformed: 0 },
  addFailed: 1,
};

test("a failed connection's details count its candidates and pairs and keep its states in order", () => {
  const seen = { ice: ['new', 'checking', 'disconnected', 'failed'], peer: ['new', 'connecting', 'failed'] };
  assert.equal(
    connectionDetails(AGENTS.Blink, seen, STATS, FACTS),
    'joiner; Blink; server cloudflare; attempt 3; ice new>checking>disconnected>failed; ' +
      'connection new>connecting>failed; gathering complete; local 1 host (1 mDNS), 1 srflx; ' +
      'code 2 host (1 mDNS, 1 address), 1 srflx; ' +
      'kept 2, dropped 1 private; adds failed 1; remote 2 host (1 address), 1 srflx, 1 unresolved; ' +
      'pairs 2 failed, 1 waiting, 1 frozen',
  );
  // the host's side names no attempts
  assert.equal(
    connectionDetails(AGENTS.Gecko, { ice: ['new', 'failed'], peer: ['new', 'failed'] }, [], {
      side: 'host',
      server: 'lan',
      gathering: 'complete',
      code: [{ type: 'host', ip: NAME, port: 1 }],
      kept: 1,
      dropped: {},
      addFailed: 0,
    }),
    'host; Gecko; server lan; ice new>failed; connection new>failed; gathering complete; local none; ' +
      'code 1 host (1 mDNS); kept 1, dropped none; adds failed 0; remote none; pairs none',
  );
});

test("the candidate filter keeps what the rules take and says why it dropped the rest", () => {
  const data = {
    candidates: [
      { type: 'srflx', ip: '93.184.216.34', port: 61000 },
      { type: 'srflx', ip: '151.101.1.69', port: 61001 },
      { type: 'host', ip: NAME, port: 54321 },
      { type: 'host', ip: '192.168.1.20', port: 50001 },
      { type: 'host', ip: '127.0.0.1', port: 50002 },
      { type: 'host', ip: 'other.local', port: 50003 },
      { type: 'host', ip: '10.0.0.5', port: 0 },
    ],
  };
  // a host's own public address (a joiner behind the same router has it too) and a private one on a public server
  const own = new Set(['151.101.1.69']);
  const out = filterCandidates(data, false, own);
  assert.equal(out.kept, 2);
  assert.equal(out.lines.length, 2);
  assert.ok(out.lines[0].includes(' 93.184.216.34 61000 typ srflx'));
  assert.ok(out.lines[1].includes(NAME));
  assert.deepEqual(out.dropped, { own: 1, private: 1, reserved: 1, malformed: 2 });
  // on the same-network server the private address is kept; the page's own mDNS name never is
  const lan = filterCandidates(data, true, new Set([NAME]));
  assert.equal(lan.kept, 3);
  assert.deepEqual(lan.dropped, { own: 1, private: 0, reserved: 1, malformed: 2 });
});

test("a failed connection's details never hold an address, port, name, ufrag or anything the stats make up", () => {
  const odd = [
    ...STATS,
    { type: 'local-candidate', candidateType: '<b>relay</b>', address: '10.0.0.1' },
    { type: 'candidate-pair', state: 'stolen 10.0.0.2' },
    { type: 'remote-candidate', candidateType: 'prflx', address: 'evil.local' },
  ];
  const seen = { ice: ['new', 'checking 10.0.0.3', '<script>'], peer: undefined };
  const facts = {
    side: 'joiner 10.0.0.5',
    server: 'stun.evil.example',
    attempts: '7 10.0.0.6',
    gathering: 'gathering 10.0.0.7',
    code: [...CODE, { type: '<i>srflx</i>', ip: 'evil.local', port: 9 }, null],
    kept: -1,
    dropped: { own: 1.5, private: 2, '10.0.0.8': 4, malformed: 'evil' },
    addFailed: Infinity,
  };
  const text = connectionDetails(`${AGENTS.Gecko} 10.0.0.4`, seen, odd, facts);
  assert.equal(
    text,
    'other; Gecko; server other; ice new>other>other; connection none; gathering other; ' +
      'local 1 host (1 mDNS), 1 srflx, 1 other; code 2 host (1 mDNS, 1 address), 1 srflx, 2 other; ' +
      'kept 0, dropped 2 private; adds failed 0; ' +
      'remote 2 host (1 address), 1 srflx, 1 prflx, 1 unresolved; pairs 2 failed, 1 waiting, 1 frozen, 1 other',
  );
  const leaks = ['.local', '203.0', '198.51', '192.168', '10.0', '5432', '61000', 'AbCd', '<', 'evil', 'stolen'];
  leaks.push('example');
  for (const bad of leaks) {
    assert.ok(!text.includes(bad), `${bad} in "${text}"`);
  }
});

test('a connection with no stats says so', () => {
  assert.equal(
    connectionDetails(AGENTS.WebKit, { ice: ['new'], peer: ['new'] }, [], {
      side: 'joiner',
      server: 'google',
      attempts: 1,
      gathering: 'new',
    }),
    'joiner; WebKit; server google; attempt 1; ice new; connection new; gathering new; local none; code none; ' +
      'kept 0, dropped none; adds failed 0; remote none; pairs none',
  );
  assert.equal(
    connectionDetails('', null, null),
    'other; other; server other; ice none; connection none; gathering none; local none; code none; ' +
      'kept 0, dropped none; adds failed 0; remote none; pairs none',
  );
});

test('a time for the player reads in whole minutes', () => {
  assert.equal(minutes(15 * 60_000), '15 minutes');
  assert.equal(minutes(5 * 60_000), '5 minutes');
  assert.equal(minutes(60_000), '1 minute');
});
