// The page's runner of client scripts: each realm's guest (`torque_lua_client.wasm`, `just lua-client-web`) in a Worker of
// its own (client-script-worker.js), so a script never blocks the game's frame. The game's module reaches it as
// `globalThis.torqueClientScript` (torque-client-script-host's `WebRunner`), which a page that allows server scripts sets to
// `await clientScripts({ guest })` before the game starts. Runs in a browser and under Node (worker_threads).
//
// `open(wasm, deadlineMs, memoryBytes)` makes a realm's Worker over `wasm` (empty: the page's guest): the module's memory
// maximum is first set to `memoryBytes` (a module that imports its memory, shares it or has more than one is refused), so a
// growth past the cap fails inside the guest. The realm's `start(kind, nowUs, a, b, c)` posts one entry (0 new, 1 add a
// script, 2 run, 3 frame, with its byte arguments) and `take()` hands back its outcome once there is one:
// `{ status, ops, pending }`, or `{ trap: 'deadline' | 'memory' | 'fault', detail }`. A watchdog terminates the Worker when
// an entry runs past `deadlineMs`; after any trap the Worker is gone and every later entry has the same outcome.
// `settled()` resolves when an entry in flight ends; `live()` counts the Workers that exist. A message from a Worker
// after its realm is gone (a reply that crossed the watchdog's trap) is ignored: the trap stands.

/** How long a Worker may take to compile and start its guest. */
const LOAD_MS = 30_000;

/** The cap on a flush (the format's MAX_FLUSH_BYTES). */
export const MAX_FLUSH_BYTES = 4 << 20;

const PAGE = 65536;

function leb(bytes, at) {
  let value = 0;
  let shift = 0;
  for (;;) {
    const b = bytes[at++];
    if (b === undefined || shift > 28) throw new Error('a malformed module');
    value += (b & 0x7f) * 2 ** shift;
    if (!(b & 0x80)) return [value, at];
    shift += 7;
  }
}

function lebBytes(value) {
  const out = [];
  do {
    let b = value % 128;
    value = Math.floor(value / 128);
    if (value) b |= 0x80;
    out.push(b);
  } while (value);
  return out;
}

function skipName(bytes, at) {
  const [len, next] = leb(bytes, at);
  return next + len;
}

function skipLimits(bytes, at) {
  const flags = bytes[at++];
  [, at] = leb(bytes, at);
  if (flags & 1) [, at] = leb(bytes, at);
  return at;
}

/** Whether a module's import section imports a memory. */
function importsMemory(bytes, at, end) {
  let count;
  [count, at] = leb(bytes, at);
  for (let i = 0; i < count && at < end; i++) {
    at = skipName(bytes, skipName(bytes, at));
    const kind = bytes[at++];
    if (kind === 0) [, at] = leb(bytes, at);
    else if (kind === 1) at = skipLimits(bytes, at + 1);
    else if (kind === 2) return true;
    else if (kind === 3) at += 2;
    else if (kind === 4) [, at] = leb(bytes, at + 1);
    else throw new Error('an import of no kind');
  }
  return false;
}

/** The module `bytes` with its one memory's maximum at most `maxBytes`. */
export function withMemoryMax(bytes, maxBytes) {
  bytes = new Uint8Array(bytes);
  if (bytes.length < 8 || bytes[0] !== 0 || bytes[1] !== 0x61 || bytes[2] !== 0x73 || bytes[3] !== 0x6d) {
    throw new Error('not a WebAssembly module');
  }
  const pages = Math.floor(maxBytes / PAGE);
  let at = 8;
  while (at < bytes.length) {
    const id = bytes[at];
    const [size, body] = leb(bytes, at + 1);
    const end = body + size;
    if (end > bytes.length) throw new Error('a malformed module');
    if (id === 2 && importsMemory(bytes, body, end)) throw new Error('a guest that imports its memory');
    if (id === 5) {
      let [count, p] = leb(bytes, body);
      if (count !== 1) throw new Error('a guest with more than one memory');
      const flags = bytes[p++];
      if (flags & ~1) throw new Error('a shared or 64-bit memory');
      let min;
      let max = Infinity;
      [min, p] = leb(bytes, p);
      if (flags & 1) [max, p] = leb(bytes, p);
      if (min > pages) throw new Error('a guest whose memory starts past its cap');
      const payload = [...lebBytes(1), 1, ...lebBytes(min), ...lebBytes(Math.min(max, pages))];
      const section = [5, ...lebBytes(payload.length), ...payload];
      const out = new Uint8Array(bytes.length - (end - at) + section.length);
      out.set(bytes.subarray(0, at), 0);
      out.set(section, at);
      out.set(bytes.subarray(end), at + section.length);
      return out;
    }
    at = end;
  }
  return bytes;
}

/** A Worker running `url`, in a browser or under Node, behind one small interface. */
function spawner(NodeWorker) {
  if (NodeWorker) {
    return (url) => {
      const w = new NodeWorker(url);
      return {
        post: (message, transfer) => w.postMessage(message, transfer),
        listen: (onMessage, onError) => {
          w.on('message', onMessage);
          w.on('error', onError);
        },
        kill: () => w.terminate(),
      };
    };
  }
  return (url) => {
    const w = new Worker(url, { type: 'module' });
    return {
      post: (message, transfer) => w.postMessage(message, transfer),
      listen: (onMessage, onError) => {
        w.onmessage = (e) => onMessage(e.data);
        w.onerror = (e) => {
          e.preventDefault();
          onError(e.message ?? String(e));
        };
      },
      kill: () => w.terminate(),
    };
  };
}

/**
 * The runner over the page's realm guest `guest` (its bytes). `spawn` (for tests) makes a Worker's interface from its
 * URL, in place of a real Worker.
 */
export async function clientScripts({ guest, workerUrl = new URL('./client-script-worker.js', import.meta.url), spawn }) {
  if (!spawn) {
    const NodeWorker = typeof Worker === 'undefined' ? (await import('node:worker_threads')).Worker : null;
    spawn = spawner(NodeWorker);
  }
  const pageGuest = new Uint8Array(guest);
  let live = 0;
  let inFlight = 0;
  let waiters = [];
  const settle = () => {
    const ready = waiters;
    waiters = [];
    for (const resolve of ready) resolve();
  };

  function open(wasm, deadlineMs, memoryBytes) {
    let source;
    let refused = null;
    try {
      source = withMemoryMax(wasm.length ? wasm : pageGuest, memoryBytes);
    } catch (error) {
      refused = { trap: 'fault', detail: String(error.message ?? error) };
    }
    let worker = null;
    let ready = false;
    let queued = null;
    let busy = false;
    let outcome;
    let gone = refused;
    let timer = null;

    const finish = (result) => {
      clearTimeout(timer);
      timer = null;
      outcome = result;
      if (busy) {
        busy = false;
        inFlight--;
        settle();
      }
    };
    const kill = (result) => {
      if (!gone) {
        gone = result;
        if (worker) {
          worker.kill();
          worker = null;
          live--;
        }
      }
      if (busy) finish(gone);
    };
    const post = (call) => {
      worker.post(call, [call.a.buffer, call.b.buffer, call.c.buffer]);
      timer = setTimeout(() => kill({ trap: 'deadline' }), deadlineMs);
    };

    if (!gone) {
      worker = spawn(workerUrl);
      live++;
      worker.listen(
        (message) => {
          if (gone) return;
          if (message.type === 'ready') {
            ready = true;
            clearTimeout(timer);
            timer = null;
            if (queued) post(queued);
            queued = null;
          } else if (message.type === 'reply') {
            finish({ status: message.status, ops: message.ops, pending: message.pending, ms: message.ms });
          } else if (message.type === 'trap') {
            kill({ trap: message.trap, detail: message.detail });
          }
        },
        (error) => {
          if (!gone) kill({ trap: 'fault', detail: String(error?.message ?? error) });
        },
      );
      worker.post({ type: 'load', wasm: source }, [source.buffer]);
      timer = setTimeout(() => kill({ trap: 'fault', detail: 'the guest did not start in time' }), LOAD_MS);
    }

    return {
      start(kind, nowUs, a, b, c) {
        busy = true;
        inFlight++;
        if (gone) {
          finish(gone);
          return;
        }
        // Copies: the arguments are views into the game module's memory.
        const call = { type: 'call', kind, now: nowUs, a: a.slice(), b: b.slice(), c: c.slice() };
        if (ready) post(call);
        else queued = call;
      },
      take() {
        const result = outcome;
        outcome = undefined;
        return result;
      },
      close() {
        kill({ trap: 'fault', detail: 'the realm was closed' });
      },
    };
  }

  return {
    open,
    live: () => live,
    settled: () => (inFlight === 0 ? Promise.resolve() : new Promise((resolve) => waiters.push(resolve))),
  };
}
