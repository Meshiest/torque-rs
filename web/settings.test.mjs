// The page's settings without a page: `node --test web/settings.test.mjs`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { KEY, QUALITIES, SCALES, defaults, isGecko, load, save } from './settings.js';

// A Gecko browser's user agent, its product token left out: the page goes by `Gecko/<date>`.
const GECKO = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:145.0) Gecko/20100101';
const BLINK =
  'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Safari/537.36';

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

test('a Gecko browser starts at Low and the others at High until the player picks', () => {
  assert.ok(isGecko(GECKO));
  assert.ok(!isGecko(BLINK));
  assert.deepEqual(defaults(GECKO), { quality: 'low', picked: false, fps: false, scale: 'auto' });
  assert.deepEqual(defaults(BLINK), { quality: 'high', picked: false, fps: false, scale: 'auto' });
  assert.deepEqual(QUALITIES, ['low', 'medium', 'high', 'ultra']);
  assert.deepEqual(SCALES, ['auto', '1', '0.75', '0.5']);
});

test('the settings come back on the next visit', () => {
  const s = storage();
  assert.ok(save(s, { quality: 'ultra', picked: true, fps: true, scale: '0.75' }));
  assert.deepEqual(load(s, GECKO), { quality: 'ultra', picked: true, fps: true, scale: '0.75' });
});

test('a quality the player never picked is not kept, so a Gecko browser stays at its own', () => {
  const s = storage();
  save(s, { ...defaults(BLINK), fps: true });
  assert.deepEqual(load(s, GECKO), { quality: 'low', picked: false, fps: true, scale: 'auto' });
});

test('storage that throws or holds something else gives the defaults', () => {
  assert.deepEqual(load(storage(true), GECKO), defaults(GECKO));
  assert.equal(save(storage(true), defaults(BLINK)), false);
  assert.deepEqual(load(undefined, BLINK), defaults(BLINK));
  const s = storage();
  s.items.set(KEY, '{not json');
  assert.deepEqual(load(s, BLINK), defaults(BLINK));
  s.items.set(KEY, JSON.stringify({ quality: 'max', fps: 'yes', scale: '2' }));
  assert.deepEqual(load(s, BLINK), defaults(BLINK));
  s.items.set(KEY, '42');
  assert.deepEqual(load(s, BLINK), defaults(BLINK));
});
