// One realm's guest, in a Worker (client-script.js starts it). It answers the guest's WASI imports as the native host does:
// no files, no environment, no arguments, no exit; the guest's own clocks (one per instance, from when it started, in
// steps of 1 µs), random bytes, and standard output and error to the console, rate limited and with hidden characters
// as spaces. Nothing here calls the guest from inside an import. After a trap it answers nothing again (the page
// terminates it).

const port = typeof self === 'undefined' ? (await import('node:worker_threads')).parentPort : self;
const listen = (f) => (typeof self === 'undefined' ? port.on('message', f) : (self.onmessage = (e) => f(e.data)));

const OK = 0;
const ERRNO_BADF = 8;
const ERRNO_INVAL = 28;
const ERRNO_NOTSUP = 58;
const ERRNO_NOTCAPABLE = 76;
const MONOTONIC = 1;
const PROCESS_CPUTIME = 2;
const THREAD_CPUTIME = 3;
const MAX_TEXTS = 1024;
const MAX_TEXT_BYTES = 4 << 20;
const MAX_LINE = 4096;
const LINES_PER_SECOND = 20;
const MAX_FLUSH_BYTES = 4 << 20;
const FORMAT_VERSION = 2;

let exports = null;
let started = 0;
let broken = false;
const memory = () => exports.memory.buffer;
const view = () => new DataView(memory());

// Control characters, separators, direction controls, zero-width and tag characters (the format's `is_hidden`).
const HIDDEN = /[\u0000-\u001f\u007f-\u009f\u061c\u200b-\u200f\u2028-\u202e\u2060-\u2064\u2066-\u2069\ufeff]|\udb40[\udc00-\udc7f]/g;
let partial = '';
let second = 0;
let lines = 0;
const decoder = new TextDecoder();
function print(text) {
  partial += text;
  let at;
  while ((at = partial.indexOf('\n')) >= 0 || partial.length > MAX_LINE) {
    const end = at >= 0 ? at : MAX_LINE;
    const line = partial.slice(0, end);
    partial = partial.slice(at >= 0 ? end + 1 : end);
    const now = performance.now();
    if (now - second >= 1000) {
      second = now;
      lines = 0;
    }
    if (lines++ < LINES_PER_SECOND) console.error(`torque_lua_client.wasm: ${line.replace(HIDDEN, ' ')}`);
  }
}

const isClock = (id) => id === MONOTONIC || id === PROCESS_CPUTIME || id === THREAD_CPUTIME;
const inMemory = (ptr, len) => ptr + len <= memory().byteLength;
const fail = (why) => {
  throw new WebAssembly.RuntimeError(why);
};

const wasi = {
  args_get: () => OK,
  args_sizes_get: (count, size) => {
    view().setUint32(count, 0, true);
    view().setUint32(size, 0, true);
    return OK;
  },
  environ_get: () => OK,
  environ_sizes_get: (count, size) => {
    view().setUint32(count, 0, true);
    view().setUint32(size, 0, true);
    return OK;
  },
  clock_res_get: (id, out) => {
    if (!isClock(id)) return ERRNO_NOTSUP;
    view().setBigUint64(out, 1000n, true);
    return OK;
  },
  clock_time_get: (id, _precision, out) => {
    if (!isClock(id)) return ERRNO_NOTSUP;
    const micros = Math.floor((performance.now() - started) * 1000);
    view().setBigUint64(out, BigInt(micros) * 1000n, true);
    return OK;
  },
  fd_write: (fd, iovs, count, written) => {
    if (fd !== 1 && fd !== 2) return ERRNO_BADF;
    if (count > MAX_TEXTS) return ERRNO_INVAL;
    if (!inMemory(iovs >>> 0, count * 8)) fail('a list outside memory');
    const v = view();
    let total = 0;
    const pieces = [];
    for (let i = 0; i < count; i++) {
      const ptr = v.getUint32(iovs + 8 * i, true);
      const len = v.getUint32(iovs + 8 * i + 4, true);
      if (!inMemory(ptr, len)) fail('a buffer outside memory');
      total += len;
      if (total > MAX_TEXT_BYTES) return ERRNO_INVAL;
      pieces.push([ptr, len]);
    }
    for (const [ptr, len] of pieces) print(decoder.decode(new Uint8Array(memory(), ptr, len)));
    v.setUint32(written, total, true);
    return OK;
  },
  fd_close: () => ERRNO_BADF,
  fd_fdstat_get: () => ERRNO_BADF,
  fd_filestat_get: () => ERRNO_BADF,
  fd_prestat_get: () => ERRNO_BADF,
  fd_prestat_dir_name: () => ERRNO_BADF,
  fd_read: () => ERRNO_BADF,
  fd_seek: () => ERRNO_BADF,
  path_filestat_get: () => ERRNO_NOTCAPABLE,
  path_open: () => ERRNO_NOTCAPABLE,
  proc_exit: (code) => fail(`the guest exited (${code})`),
  random_get: (ptr, len) => {
    ptr >>>= 0;
    len >>>= 0;
    if (!inMemory(ptr, len)) fail('random bytes outside memory');
    if (len > MAX_TEXT_BYTES) return ERRNO_INVAL;
    for (let at = 0; at < len; at += 65536) {
      crypto.getRandomValues(new Uint8Array(memory(), ptr + at, Math.min(65536, len - at)));
    }
    return OK;
  },
  sched_yield: () => OK,
};

/** The trap a thrown `error` is: `memory` when the guest's memory is within 16 MiB of its cap (the growth that failed
 * was refused there; the instance is dead, so growing it now costs nothing). */
function trapOf(error) {
  try {
    exports.memory.grow(256);
  } catch {
    return { trap: 'memory' };
  }
  return { trap: 'fault', detail: String(error?.message ?? error) };
}

function put(bytes) {
  const ptr = exports.torque_realm_alloc(bytes.length);
  if (ptr === 0) throw Object.assign(new Error('no room'), { noRoom: true });
  new Uint8Array(memory(), ptr, bytes.length).set(bytes);
  return [ptr, bytes.length];
}

function call({ kind, now, a, b, c }) {
  const time = BigInt(now);
  const args = [[a], [a, b, c], [a, b], [a]][kind];
  if (!args) fail('an entry of no kind');
  const placed = args.map(put);
  const flat = placed.flat();
  const started = performance.now();
  let status;
  if (kind === 0) status = exports.torque_realm_new(...flat);
  else if (kind === 1) status = exports.torque_realm_add_script(...flat);
  else if (kind === 2) status = exports.torque_realm_run(time, ...flat);
  else status = exports.torque_realm_frame(time, ...flat);
  const ms = performance.now() - started;
  for (const [ptr, len] of placed) exports.torque_realm_free(ptr, len);
  let ops = new Uint8Array(0);
  let pending = 0;
  if (kind >= 2) {
    const len = exports.torque_realm_ops_len() >>> 0;
    if (len > MAX_FLUSH_BYTES) fail('a flush longer than the format allows');
    const ptr = exports.torque_realm_ops() >>> 0;
    if (!inMemory(ptr, len)) fail('a flush outside memory');
    ops = new Uint8Array(memory(), ptr, len).slice();
    pending = exports.torque_realm_pending() >>> 0;
  }
  return { type: 'reply', status, ops, pending, ms };
}

listen(async (message) => {
  if (broken) return;
  try {
    if (message.type === 'load') {
      const { instance } = await WebAssembly.instantiate(message.wasm, { wasi_snapshot_preview1: wasi });
      exports = instance.exports;
      started = performance.now();
      exports._initialize?.();
      if (exports.torque_realm_format() !== FORMAT_VERSION) fail('a guest of another format');
      port.postMessage({ type: 'ready' });
    } else if (message.type === 'call') {
      const reply = call(message);
      port.postMessage(reply, [reply.ops.buffer]);
    }
  } catch (error) {
    broken = true;
    const trap = error?.noRoom ? { trap: 'memory' } : exports ? trapOf(error) : { trap: 'fault', detail: String(error) };
    port.postMessage({ type: 'trap', ...trap });
  }
});

export {};
