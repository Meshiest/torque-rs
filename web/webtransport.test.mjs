// The WebTransport join's parts that need no browser: the session's address from the server's file and the page's host, and
// the reasons the page stays on its game socket.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { OPEN_TIMEOUT_MS, joinOverWebTransport, sessionUrl } from './webtransport.js';

test('the session address is the page host at the server QUIC port over https', () => {
  const info = { port: 28000, path: '/', hashes: ['00'.repeat(32)] };
  assert.equal(sessionUrl(info, { hostname: 'localhost' }), 'https://localhost:28000/');
  assert.equal(sessionUrl({ ...info, port: 4433, path: '/game' }, { hostname: '192.168.1.5' }), 'https://192.168.1.5:4433/game');
  assert.equal(sessionUrl(info, { hostname: '::1' }), 'https://[::1]:28000/');
});

test('without the game or the API the page says why and never throws', async () => {
  const marks = [];
  const mark = (why) => marks.push(why);
  assert.equal(await joinOverWebTransport({}, { mark }), 'no-game-support');
  const game = { webTransportOpened() {}, webTransportDatagram() {}, webTransportEnded() {} };
  // Node has no WebTransport: the next reason.
  assert.equal(await joinOverWebTransport(game, { mark, timeoutMs: 10 }), 'no-api');
  assert.deepEqual(marks, ['no-game-support', 'no-api']);
  assert.ok(OPEN_TIMEOUT_MS >= 1000);
});
