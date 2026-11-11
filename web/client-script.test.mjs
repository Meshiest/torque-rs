// The page's runner of client scripts (client-script.js) with stand-in Workers, without a guest: `node --test
// web/client-script.test.mjs`. The guest's own cases run in real Workers in `client-script-check.mjs`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { clientScripts, startPageRunner, withMemoryMax } from './client-script.js';

// A module of one memory of one page and nothing else.
const MODULE = Uint8Array.from([0, 0x61, 0x73, 0x6d, 1, 0, 0, 0, 5, 3, 1, 0, 1]);

/** A stand-in Worker that starts at once and answers only when the test says so. */
function standIn() {
  const worker = { calls: [], killed: false };
  const spawn = () => ({
    post: (message) => {
      if (message.type === 'load') queueMicrotask(() => worker.onMessage({ type: 'ready' }));
      else worker.calls.push(message);
    },
    listen: (onMessage, onError) => {
      worker.onMessage = onMessage;
      worker.onError = onError;
    },
    kill: () => {
      worker.killed = true;
    },
  });
  return { worker, spawn };
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const none = new Uint8Array(0);

test('a reply that crosses the watchdog leaves the trap standing', async () => {
  const { worker, spawn } = standIn();
  const scripts = await clientScripts({ guest: MODULE, unchecked: true, spawn });
  const realm = scripts.open(none, 20, 1 << 20);
  await sleep(0);
  realm.start(3, 0, none, none, none);
  await scripts.settled();
  assert.deepEqual(realm.take(), { trap: 'deadline' });
  assert.ok(worker.killed);
  assert.equal(scripts.live(), 0);
  // The guest's reply, posted as the watchdog fired, arrives after it.
  worker.onMessage({ type: 'reply', status: 0, ops: new Uint8Array(0), pending: 0, ms: 20 });
  worker.onError('late');
  assert.equal(realm.take(), undefined, 'nothing replaces the trap');
  realm.start(3, 0, none, none, none);
  assert.deepEqual(realm.take(), { trap: 'deadline' }, 'and every later entry has it');
});

test('a reply in time is the outcome, and the watchdog does not fire after it', async () => {
  const { worker, spawn } = standIn();
  const scripts = await clientScripts({ guest: MODULE, unchecked: true, spawn });
  const realm = scripts.open(none, 30, 1 << 20);
  await sleep(0);
  realm.start(3, 0, none, none, none);
  worker.onMessage({ type: 'reply', status: 0, ops: new Uint8Array([1]), pending: 2, ms: 1 });
  assert.deepEqual(realm.take(), { status: 0, ops: new Uint8Array([1]), pending: 2, ms: 1 });
  await sleep(60);
  assert.ok(!worker.killed);
  assert.equal(scripts.live(), 1);
  realm.close();
  assert.equal(scripts.live(), 0);
});

test('the memory maximum is set to the cap, and imported or shared memories are refused', () => {
  const capped = withMemoryMax(MODULE, 2 << 16);
  assert.deepEqual([...capped.subarray(8)], [5, 4, 1, 1, 1, 2]);
  const imported = Uint8Array.from([0, 0x61, 0x73, 0x6d, 1, 0, 0, 0, 2, 7, 1, 1, 0x6d, 1, 0x6d, 2, 0, 1]);
  assert.throws(() => withMemoryMax(imported, 1 << 20), /imports its memory/);
  const shared = Uint8Array.from([0, 0x61, 0x73, 0x6d, 1, 0, 0, 0, 5, 4, 1, 3, 1, 1]);
  assert.throws(() => withMemoryMax(shared, 1 << 20), /shared/);
  assert.throws(() => withMemoryMax(MODULE, 1 << 15), /past its cap/);
});

test('a guest runs only if its hash is on the list the build gives', async () => {
  const { spawn } = standIn();
  // The SHA-256 of MODULE, from the platform's own digest.
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', MODULE));
  const hash = Array.from(digest, (b) => b.toString(16).padStart(2, '0')).join('');
  const scripts = await clientScripts({ guest: MODULE, accepted: [hash], spawn });
  assert.equal(scripts.live(), 0);
  await assert.rejects(clientScripts({ guest: MODULE, accepted: [], spawn }), /is not one this build accepts/);
  await assert.rejects(clientScripts({ guest: MODULE, accepted: ['0'.repeat(64)], spawn }), /is not one this build accepts/);
  const changed = Uint8Array.from(MODULE);
  changed[changed.length - 1] ^= 1;
  await assert.rejects(clientScripts({ guest: changed, accepted: [hash], spawn }), /is not one this build accepts/);
});

test('a call with no list and no explicit opt-out is refused', async () => {
  const { spawn } = standIn();
  await assert.rejects(clientScripts({ guest: MODULE, spawn }), /needs the list of accepted guest hashes/);
  await assert.rejects(clientScripts({ guest: MODULE, accepted: 'abc', spawn }), /needs the list of accepted guest hashes/);
  await assert.rejects(clientScripts({ guest: MODULE, accepted: undefined, unchecked: false, spawn }), /needs the list of accepted guest hashes/);
});

/** A response of `bytes`, or of `status` alone. */
const answer = (bytes, status = 200) => ({ ok: status === 200, status, arrayBuffer: async () => bytes.buffer });

test('the page runner starts only for a game with a list, on a guest the list holds', async () => {
  const { spawn } = standIn();
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', MODULE));
  const hash = Array.from(digest, (b) => b.toString(16).padStart(2, '0')).join('');
  const listing = (list) => ({ acceptedClientGuests: () => list });
  const run = async (game, response) => {
    const target = {};
    const why = await startPageRunner(game, 'guest.wasm', { get: async () => response, target, spawn });
    return { why, runner: target.torqueClientScript };
  };

  let { why, runner } = await run({}, answer(MODULE));
  assert.match(why, /lists no realm guest/);
  assert.equal(runner, undefined, 'a module with no list never gets the runner');

  ({ why, runner } = await run(listing([hash]), answer(new Uint8Array(0), 404)));
  assert.match(why, /could not be fetched \(HTTP 404\)/);
  assert.equal(runner, undefined);

  ({ why, runner } = await run(listing(['0'.repeat(64)]), answer(MODULE)));
  assert.match(why, /is not one this build accepts/);
  assert.equal(runner, undefined, 'a guest off the list is never run');

  ({ why, runner } = await run(listing([hash]), answer(MODULE)));
  assert.equal(why, '');
  assert.equal(typeof runner.open, 'function', 'the runner the module opens realms with');
  assert.equal(runner.live(), 0);

  const failing = { acceptedClientGuests: () => [hash] };
  const target = {};
  why = await startPageRunner(failing, 'guest.wasm', {
    get: async () => {
      throw new Error('offline');
    },
    target,
    spawn,
  });
  assert.match(why, /could not be fetched: offline/);
  assert.equal(target.torqueClientScript, undefined);
});
