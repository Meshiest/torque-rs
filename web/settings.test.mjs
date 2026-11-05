// The page's settings without a page: `node --test web/settings.test.mjs`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { KEY, QUALITIES, defaults, isFirefox, load, save } from './settings.js';

const FIREFOX = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:145.0) Gecko/20100101 Firefox/145.0';
const CHROME =
  'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36';

// A `localStorage` in memory; `broken` makes every call throw (storage blocked or full).
function storage(broken = false) {
  const items = new Map();
  return {
    items,
    getItem(k) {
      if (broken) throw new Error('blocked');
      return items.has(k) ? items.get(k) : null;
    },
    setItem(k, v) {
      if (broken) throw new Error('blocked');
      items.set(k, String(v));
    },
  };
}

test('Firefox starts at Low and the others at High until the player picks', () => {
  assert.ok(isFirefox(FIREFOX));
  assert.ok(!isFirefox(CHROME));
  assert.deepEqual(defaults(FIREFOX), { quality: 'low', picked: false, fps: false });
  assert.deepEqual(defaults(CHROME), { quality: 'high', picked: false, fps: false });
  assert.deepEqual(QUALITIES, ['low', 'medium', 'high', 'ultra']);
});

test('the settings come back on the next visit', () => {
  const s = storage();
  assert.ok(save(s, { quality: 'ultra', picked: true, fps: true }));
  assert.deepEqual(load(s, FIREFOX), { quality: 'ultra', picked: true, fps: true });
});

test('a quality the player never picked is not kept, so Firefox stays at its own', () => {
  const s = storage();
  save(s, { ...defaults(CHROME), fps: true });
  assert.deepEqual(load(s, FIREFOX), { quality: 'low', picked: false, fps: true });
});

test('storage that throws or holds something else gives the defaults', () => {
  assert.deepEqual(load(storage(true), FIREFOX), defaults(FIREFOX));
  assert.equal(save(storage(true), defaults(CHROME)), false);
  assert.deepEqual(load(undefined, CHROME), defaults(CHROME));
  const s = storage();
  s.items.set(KEY, '{not json');
  assert.deepEqual(load(s, CHROME), defaults(CHROME));
  s.items.set(KEY, JSON.stringify({ quality: 'max', fps: 'yes' }));
  assert.deepEqual(load(s, CHROME), defaults(CHROME));
  s.items.set(KEY, '42');
  assert.deepEqual(load(s, CHROME), defaults(CHROME));
});
