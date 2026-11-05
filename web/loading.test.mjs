// The loading display's stages and streamed reads, without a page: `node --test web/loading.test.mjs`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { STAGES, fpsText, later, percent, readStreamed, stageText } from './loading.js';

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
