// The page's `torque` object without a page or a game: `node --test web/bridge.test.mjs`. The game's side (its caps,
// registration and the requests run on the script engine) is torque-app's `page_bridge` tests.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { TorqueError, createTorque, decodeError } from './bridge.js';

// A game module that answers requests as the bridge does, at once.
function fakeGame(answer) {
  const on = new Set();
  return {
    on,
    torqueRequest(kind, a, b, args, resolve, reject) {
      const out = answer(kind, a, b, args);
      if (out instanceof Error) reject(out.message);
      else resolve(out);
    },
    torqueOn(name) {
      if (!/^[A-Za-z_]\w*$/.test(name)) throw JSON.stringify({ kind: 'bad-name', message: `${name} is not a name` });
      on.add(name);
    },
    torqueOff(name) {
      on.delete(name);
    },
  };
}

test('requests made before the game is attached wait for it', async () => {
  const torque = createTorque();
  const seen = [];
  const early = torque.call('add', 2, 3);
  torque.attach(fakeGame((kind, a, b, args) => (seen.push([kind, a, b, args]), 'ok')));
  assert.equal(await early, 'ok');
  assert.equal(await torque.setVar('$x', 5), 'ok');
  assert.equal(await torque.getVar('$x'), 'ok');
  assert.equal(await torque.eval('return 1;'), 'ok');
  assert.deepEqual(seen, [
    ['call', 'add', '', ['2', '3']],
    ['set', '$x', '5', []],
    ['get', '$x', '', []],
    ['eval', 'return 1;', '', []],
  ]);
});

test('a syntax error comes back with its line and column', async () => {
  const torque = createTorque();
  const json = JSON.stringify({ kind: 'syntax', message: 'syntax error at line 2, column 12', line: 2, column: 12 });
  torque.attach(fakeGame(() => new Error(json)));
  await assert.rejects(torque.eval('$a = 1;\n$b = 2 +;'), (e) => {
    assert.ok(e instanceof TorqueError);
    assert.equal(e.kind, 'syntax');
    assert.equal(e.line, 2);
    assert.equal(e.column, 12);
    return true;
  });
});

test('handlers registered before the game are registered with it, and only those are dispatched', () => {
  const torque = createTorque();
  torque.on('mapLoaded', (name) => `loaded ${name}`);
  const game = fakeGame(() => '');
  torque.attach(game);
  assert.deepEqual([...game.on], ['mapLoaded']);
  assert.equal(torque.dispatch('mapLoaded', ['office']), 'loaded office');
  torque.on('died', () => undefined);
  assert.equal(torque.dispatch('died', []), '');
  assert.throws(() => torque.dispatch('other', []), /no handler other/);
  torque.off('died');
  assert.deepEqual([...game.on], ['mapLoaded']);
  assert.throws(() => torque.dispatch('died', []));
  assert.throws(() => torque.on('bad name', () => {}), (e) => e.kind === 'bad-name');
  assert.throws(() => torque.dispatch('bad name', []), 'a refused name is not kept');
  assert.throws(() => torque.on('x', 'not a function'), TypeError);
});

test('a game whose page has no bridge refuses cleanly', async () => {
  const torque = createTorque();
  torque.attach({});
  await assert.rejects(torque.call('x'), (e) => e.kind === 'no-bridge');
});

test('anything else thrown becomes a failed TorqueError', () => {
  const e = decodeError(new Error('boom'));
  assert.equal(e.kind, 'failed');
  assert.equal(e.message, 'boom');
  assert.equal(decodeError('not json').kind, 'failed');
});
