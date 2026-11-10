// The page's Lua bridge against a hand-made module that names texts no real module names: `node --test web/lua.test.mjs`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { MAX_NAME, MAX_VALUE, createLua } from './lua.js';

const RANGE = 6;

const u32 = (n) => {
  const out = [];
  do {
    let byte = n & 0x7f;
    n >>>= 7;
    if (n) byte |= 0x80;
    out.push(byte);
  } while (n);
  return out;
};
const vec = (items) => [...u32(items.length), ...items.flat()];
const name = (s) => vec([...new TextEncoder().encode(s)].map((b) => [b]));
const section = (id, body) => [id, ...u32(body.length), ...body];
const I32 = 0x7f;
const I64 = 0x7e;
const type = (params, results) => [0x60, ...vec(params.map((p) => [p])), ...vec(results.map((r) => [r]))];
const body = (code) => [...u32(code.length + 1), 0x00, ...code];

// A module whose `torque_lua_run_file` makes one request with its call's handle and answers its status: `get_global` of
// the `side` bytes at 0 when the source is empty, else `set_global` of a one-byte name and a value of `side` bytes.
function hostileModule() {
  const types = vec([
    type([I32, I32, I32], [I32]), // get_global
    type([I32, I32, I32, I32, I32], [I32]), // set_global
    type([], []), // _initialize
    type([], [I32]), // torque_lua_start
    type([I32], [I32]), // torque_lua_alloc
    type([I32, I32], []), // torque_lua_free
    type([I32, I64, I32, I32, I32, I32, I32, I32, I32], [I32]), // torque_lua_run_file
  ]);
  const imports = vec([
    [...name('torque'), ...name('get_global'), 0x00, 0],
    [...name('torque'), ...name('set_global'), 0x00, 1],
  ]);
  const functions = vec([[2], [3], [4], [5], [6]]);
  const memory = vec([[0x00, 1]]);
  const exports = vec([
    [...name('memory'), 0x02, 0],
    [...name('_initialize'), 0x00, 2],
    [...name('torque_lua_start'), 0x00, 3],
    [...name('torque_lua_alloc'), 0x00, 4],
    [...name('torque_lua_free'), 0x00, 5],
    [...name('torque_lua_run_file'), 0x00, 6],
  ]);
  const code = vec([
    body([0x0b]),
    body([0x41, 0x00, 0x0b]),
    body([0x41, ...u32(1024), 0x0b]),
    body([0x0b]),
    // if (source length) set_global(handle, 0, 1, 0, side) else get_global(handle, 0, side)
    body([
      0x20, 6, 0x04, I32,
      0x20, 0, 0x41, 0, 0x41, 1, 0x41, 0, 0x20, 2, 0x10, 1,
      0x05,
      0x20, 0, 0x41, 0, 0x20, 2, 0x10, 0,
      0x0b, 0x0b,
    ]),
  ]);
  return new Uint8Array([
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00,
    ...section(1, types),
    ...section(2, imports),
    ...section(3, functions),
    ...section(5, memory),
    ...section(7, exports),
    ...section(10, code),
  ]);
}

test('texts past the C ABI caps are refused before they are copied or reach the game', async () => {
  const lua = await createLua(await WebAssembly.compile(hostileModule()));
  let requests = 0;
  const host = () => {
    requests += 1;
    return 0;
  };
  const none = new Uint8Array(0);
  const one = new Uint8Array(1);
  // A name longer than the module's whole memory, one past the name cap, a value one past the value cap.
  assert.equal(lua.runFile(host, 1, 1 << 24, none, none, none), RANGE);
  assert.equal(lua.runFile(host, 1, MAX_NAME + 1, none, none, none), RANGE);
  assert.equal(lua.runFile(host, 1, MAX_VALUE + 1, none, one, none), RANGE);
  assert.equal(requests, 0, 'no request reached the game');
  assert.equal(lua.broken, false);
  // Within the caps the request is the game's.
  assert.equal(lua.runFile(host, 1, 8, none, none, none), 0);
  assert.equal(requests, 1);
});
