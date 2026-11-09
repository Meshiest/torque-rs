// The page's runner of client scripts (client-script.js) with stand-in Workers, without a guest: `node --test
// web/client-script.test.mjs`. The guest's own cases run in real Workers in `client-script-check.mjs`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { clientScripts, withMemoryMax } from './client-script.js';

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
  const scripts = await clientScripts({ guest: MODULE, spawn });
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
  const scripts = await clientScripts({ guest: MODULE, spawn });
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
