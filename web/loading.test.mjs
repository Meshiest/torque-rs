// The loading display's stages and streamed reads, without a page: `node --test web/loading.test.mjs`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { STAGES, fpsText, later, loadModule, percent, readStreamed, showBar, stageText } from './loading.js';

test('the stages run in the order the page and the game set them', () => {
  assert.deepEqual(STAGES, ['module', 'compile', 'files', 'start', 'gpu', 'map', 'ready']);
  assert.ok(later('files', 'gpu'));
  assert.ok(later('gpu', 'ready'));
  assert.ok(!later('map', 'gpu'), 'a stage never goes back');
  assert.ok(!later('map', 'map'));
});

test('a stage line names its step and a download its bytes and percent', () => {
  assert.equal(stageText('compile'), 'Compiling the game (2 of 7)');
  assert.equal(stageText('module', 3 * 1048576, 12 * 1048576), 'Downloading the game (1 of 7): 3.0 of 12.0 MiB, 25%');
  assert.equal(stageText('files', 1048576, null), "Loading the game's files (3 of 7): 1.0 MiB");
  assert.equal(stageText('ready'), 'Ready (7 of 7)');
});

test('percent stays within 0 and 100', () => {
  assert.equal(percent(0, 0), 0);
  assert.equal(percent(5, 10), 50);
  assert.equal(percent(12, 10), 100);
  assert.equal(percent(-1, 10), 0);
});

// A response whose body comes in `pieces`, with `length` as its Content-Length (none: null).
function streamed(pieces, length) {
  const body = new ReadableStream({
    start(c) {
      for (const p of pieces) c.enqueue(new Uint8Array(p));
      c.close();
    },
  });
  const headers = length === null ? {} : { 'Content-Length': String(length) };
  return new Response(body, { headers });
}

test('a streamed read follows the bytes as they come', async () => {
  const seen = [];
  const bytes = await readStreamed(streamed([[1, 2, 3], [4, 5], [6]], 6), (d, t) => seen.push([d, t]));
  assert.deepEqual([...bytes], [1, 2, 3, 4, 5, 6]);
  assert.deepEqual(seen, [[0, 6], [3, 6], [5, 6], [6, 6]]);
});

test('a read without a length counts the bytes alone', async () => {
  const seen = [];
  await readStreamed(streamed([[1], [2, 3]], null), (d, t) => seen.push([d, t]));
  assert.deepEqual(seen, [[0, null], [1, null], [3, null]]);
});

test('a compressed body longer than its length is held at the length', async () => {
  const seen = [];
  const bytes = await readStreamed(streamed([[1, 2], [3, 4]], 3), (d, t) => seen.push([d, t]));
  assert.equal(bytes.length, 4);
  assert.deepEqual(seen.at(-1), [3, 3]);
});

test('a failed response is an error', async () => {
  await assert.rejects(readStreamed(new Response('no', { status: 404 })), /HTTP 404/);
});

test('the frame-time line', () => {
  assert.equal(fpsText('', ''), 'fps: waiting for frames');
  assert.equal(fpsText('59.6', '16.78 22.1'), '60 fps, 16.78 ms a frame, worst 22.1 ms');
  assert.equal(fpsText('120', '8.33'), '120 fps, 8.33 ms a frame');
});

// A loading bar as the page has it: an element with a fill (its first child), its classes and its attributes.
function fakeBar() {
  const classes = new Set(['busy']);
  const attributes = new Map();
  return {
    firstElementChild: { style: { transform: '' } },
    classList: { add: (c) => classes.add(c), remove: (c) => classes.delete(c), contains: (c) => classes.has(c) },
    setAttribute: (k, v) => attributes.set(k, v),
    removeAttribute: (k) => attributes.delete(k),
    getAttribute: (k) => attributes.get(k) ?? null,
  };
}

test('the loading bar fills by a transform and tells a screen reader the percent', () => {
  const bar = fakeBar();
  showBar(bar, 3 * 1048576, 12 * 1048576);
  assert.ok(!bar.classList.contains('busy'));
  assert.equal(bar.firstElementChild.style.transform, 'scaleX(0.25)');
  assert.equal(bar.getAttribute('aria-valuenow'), '25');
  showBar(bar, 20, 10);
  assert.equal(bar.firstElementChild.style.transform, 'scaleX(1)');
});

test('a stage of unknown length shows the moving bar with no value', () => {
  const bar = fakeBar();
  showBar(bar, 5, 10);
  showBar(bar);
  assert.ok(bar.classList.contains('busy'));
  assert.equal(bar.getAttribute('aria-valuenow'), null);
  assert.equal(bar.firstElementChild.style.transform, '');
  showBar(bar, 1048576, null);
  assert.ok(bar.classList.contains('busy'), 'bytes with no total have no share to show');
});

// The smallest module: the header alone.
const EMPTY_MODULE = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
function moduleResponse(pieces = [EMPTY_MODULE.slice(0, 3), EMPTY_MODULE.slice(3)]) {
  const body = new ReadableStream({
    start(c) {
      for (const p of pieces) c.enqueue(new Uint8Array(p));
      c.close();
    },
  });
  return new Response(body, { headers: { 'Content-Type': 'application/wasm', 'Content-Length': '8' } });
}

test('a module is compiled while it downloads, its progress followed', async () => {
  const seen = [];
  let compiling = 0;
  let fetched = 0;
  const module = await loadModule(
    'http://x/game_bg.wasm',
    (d, t) => seen.push([d, t]),
    () => compiling++,
    async () => (fetched++, moduleResponse()),
  );
  assert.ok(module instanceof WebAssembly.Module);
  assert.equal(fetched, 1, 'one download');
  assert.equal(compiling, 1);
  assert.deepEqual(seen.at(-1), [8, 8]);
});

test('a module the stream compiler refuses is downloaded again and compiled from its bytes', async () => {
  const originalFrame = globalThis.requestAnimationFrame;
  globalThis.requestAnimationFrame = (call) => setTimeout(call, 0);
  const originalWarn = console.warn;
  console.warn = () => {};
  try {
    let fetched = 0;
    // The first response's bytes are no module (a bad copy in a proxy); the second is the file.
    const module = await loadModule(
      'http://x/game_bg.wasm',
      () => {},
      () => {},
      async () => (fetched++ === 0 ? moduleResponse([[1, 2, 3, 4, 5, 6, 7, 8]]) : moduleResponse()),
    );
    assert.ok(module instanceof WebAssembly.Module);
    assert.equal(fetched, 2);
  } finally {
    globalThis.requestAnimationFrame = originalFrame;
    console.warn = originalWarn;
  }
});

test('a failed module request is an error', async () => {
  await assert.rejects(
    loadModule('http://x/game_bg.wasm', () => {}, () => {}, async () => new Response('no', { status: 404 })),
    /HTTP 404/,
  );
});
