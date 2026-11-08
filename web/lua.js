// The page's Lua: the Lua extension built as a WebAssembly module (`torque_lua.wasm`, `just lua-web`), and the bridge the
// game's module calls (`globalThis.torqueLua`, which torque-plugin-loader's `web_lua::PageLuaModule` looks for). A page loads
// it only when its game uses Lua (`loadLua`, before the game's module starts); without it a Lua file prints that it cannot
// run, and the page works as it does.
//
// The bridge: `runFile(host, vm, side, path, source, package)` and `callDefined(host, vm, side, id, args)` run a call of the
// game's script engine in the module, `realmDropped(vm)` drops a script engine's realm. `host(op, args)` is the game's
// function that serves the module's requests during that call (the engine's checks are all made there); it answers with the
// C ABI's status, or with a text (`Uint8Array`) or a number. A call answers with a status, `callDefined` with its text when
// it worked.
//
// The module's WASI imports are its C library's, and none reaches anything: no files (no folder is opened, every file call
// fails), no environment or arguments, no exit. The clocks are the page's monotonic one, for `os.clock` (the wall clock is
// refused). Random bytes come from `crypto.getRandomValues` (the hash tables' seeds). What the module writes to its own
// standard output and error (a panic's message) goes to the page's console.
//
// The module needs WebAssembly exception handling with `exnref` (Chrome and Edge 137, Firefox 131, Safari 18.4, Node 24).

const OK = 0;
const HANDLE = 3;
const PANIC = 4;
const FAILED = 5;

// The game's requests (`web_lua/page.rs`'s `op`).
const OP = {
  log: 0,
  getGlobal: 1,
  setGlobal: 2,
  callFunction: 3,
  callMethod: 4,
  hasMethod: 5,
  findObject: 6,
  getField: 7,
  setField: 8,
  defineFunction: 9,
  charge: 10,
  readScript: 11,
};

// WASI's error numbers the shim answers with.
const ERRNO_BADF = 8;
const ERRNO_NOTSUP = 58;
const ERRNO_NOTCAPABLE = 76;
// WASI's clocks: the monotonic one and the process's and thread's CPU time (C's `clock()`).
const MONOTONIC = 1;
const PROCESS_CPUTIME = 2;
const THREAD_CPUTIME = 3;

/** Loads the Lua module from `source` (a URL, its bytes or a compiled `WebAssembly.Module`) and installs the bridge. */
export async function loadLua(source = 'torque_lua.wasm') {
  let module = source;
  if (!(source instanceof WebAssembly.Module)) {
    const bytes =
      source instanceof ArrayBuffer || ArrayBuffer.isView(source)
        ? source
        : await fetch(source).then((r) => {
            if (!r.ok) throw new Error(`${source}: HTTP ${r.status}`);
            return r.arrayBuffer();
          });
    module = await WebAssembly.compile(bytes);
  }
  const lua = await createLua(module);
  globalThis.torqueLua = lua;
  return lua;
}

/** The bridge over a new instance of the compiled Lua module `module`. */
export async function createLua(module) {
  let exports = null;
  let broken = null;
  // The game's functions of the running calls, by the handle the module is given.
  const hosts = new Map();
  let nextHandle = 1;
  // The answer the module copies out next (`result_len`, `result_copy`).
  let pending = new Uint8Array(0);
  const decoder = new TextDecoder();

  const memory = () => exports.memory.buffer;
  const view = () => new DataView(memory());
  const read = (ptr, len) => new Uint8Array(memory(), ptr, len).slice();
  // The `count` texts (a pointer and a length each) at `ptr`.
  const readTexts = (ptr, count) => {
    const texts = [];
    const v = view();
    for (let i = 0; i < count; i++) texts.push(read(v.getUint32(ptr + 8 * i, true), v.getUint32(ptr + 8 * i + 4, true)));
    return texts;
  };
  const text = (ptr) => {
    const v = view();
    return read(v.getUint32(ptr, true), v.getUint32(ptr + 4, true));
  };
  // A request of the module's for the call `call`, answered by the game: a text becomes the pending answer.
  const request = (call, op, args) => {
    const host = hosts.get(call);
    if (!host) return HANDLE;
    const answer = host(op, args);
    if (answer instanceof Uint8Array) {
      pending = answer;
      return OK;
    }
    return typeof answer === 'number' ? answer : FAILED;
  };

  let lines = '';
  const MAX_LINE = 4096;
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
      if (id !== MONOTONIC && id !== PROCESS_CPUTIME && id !== THREAD_CPUTIME) return ERRNO_NOTSUP;
      view().setBigUint64(out, 1000n, true);
      return OK;
    },
    clock_time_get: (id, _precision, out) => {
      if (id !== MONOTONIC && id !== PROCESS_CPUTIME && id !== THREAD_CPUTIME) return ERRNO_NOTSUP;
      view().setBigUint64(out, BigInt(Math.round(performance.now() * 1e6)), true);
      return OK;
    },
    fd_write: (fd, iovs, count, written) => {
      if (fd !== 1 && fd !== 2) return ERRNO_BADF;
      let n = 0;
      for (const chunk of readTexts(iovs, count)) {
        lines += decoder.decode(chunk);
        n += chunk.length;
      }
      for (let at = lines.indexOf('\n'); at >= 0; at = lines.indexOf('\n')) {
        console.warn(`torque_lua.wasm: ${lines.slice(0, at)}`);
        lines = lines.slice(at + 1);
      }
      // A line that never ends is printed in pieces rather than kept growing.
      while (lines.length > MAX_LINE) {
        console.warn(`torque_lua.wasm: ${lines.slice(0, MAX_LINE)}`);
        lines = lines.slice(MAX_LINE);
      }
      view().setUint32(written, n, true);
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
    proc_exit: (code) => {
      throw new Error(`torque_lua.wasm exited (${code})`);
    },
    random_get: (ptr, len) => {
      for (let at = 0; at < len; at += 65536) {
        crypto.getRandomValues(new Uint8Array(memory(), ptr + at, Math.min(65536, len - at)));
      }
      return OK;
    },
    sched_yield: () => OK,
  };

  const torque = {
    log: (call, level, ptr, len) => {
      if (call === 0) {
        (level === 2 ? console.error : level === 1 ? console.warn : console.log)(`torque_lua.wasm: ${decoder.decode(read(ptr, len))}`);
        return OK;
      }
      return request(call, OP.log, [level, read(ptr, len)]);
    },
    get_global: (call, ptr, len) => request(call, OP.getGlobal, [read(ptr, len)]),
    set_global: (call, name, nameLen, value, valueLen) => request(call, OP.setGlobal, [read(name, nameLen), read(value, valueLen)]),
    call_function: (call, ns, nsLen, name, nameLen, args, count) =>
      request(call, OP.callFunction, [read(ns, nsLen), read(name, nameLen), readTexts(args, count)]),
    call_method: (call, object, name, nameLen, args, count) =>
      request(call, OP.callMethod, [object >>> 0, read(name, nameLen), readTexts(args, count)]),
    has_method: (call, object, name, len) => {
      const host = hosts.get(call);
      return host && host(OP.hasMethod, [object >>> 0, read(name, len)]) === 1 ? 1 : 0;
    },
    find_object: (call, name, len, out) => {
      const host = hosts.get(call);
      if (!host) return HANDLE;
      const id = host(OP.findObject, [read(name, len)]);
      if (typeof id !== 'number') return FAILED;
      view().setUint32(out, id, true);
      return OK;
    },
    get_field: (call, object, name, nameLen, index, indexLen) =>
      request(call, OP.getField, [object >>> 0, read(name, nameLen), read(index, indexLen)]),
    set_field: (call, object, name, nameLen, index, indexLen, value, valueLen) =>
      request(call, OP.setField, [object >>> 0, read(name, nameLen), read(index, indexLen), read(value, valueLen)]),
    // A `TorqueDefineDecl`: four texts, then the id (a u64 at 32).
    define_function: (call, decl) =>
      request(call, OP.defineFunction, [text(decl), text(decl + 8), text(decl + 16), text(decl + 24), Number(view().getBigUint64(decl + 32, true))]),
    charge: (call, steps) => request(call, OP.charge, [Number(steps)]),
    read_script: (call, ptr, len) => request(call, OP.readScript, [read(ptr, len)]),
    result_len: () => pending.length,
    result_copy: (ptr) => {
      new Uint8Array(memory(), ptr, pending.length).set(pending);
      pending = new Uint8Array(0);
    },
  };

  const instance = await WebAssembly.instantiate(module, { wasi_snapshot_preview1: wasi, torque });
  exports = instance.exports;
  exports._initialize();
  const started = exports.torque_lua_start();
  if (started !== OK) throw new Error(`torque_lua.wasm did not start (status ${started})`);

  // Copies `bytes` into the module: a pointer and a length, freed by `free`.
  const put = (bytes) => {
    const ptr = exports.torque_lua_alloc(bytes.length);
    new Uint8Array(memory(), ptr, bytes.length).set(bytes);
    return [ptr, bytes.length];
  };
  const free = (texts) => {
    if (!broken) for (const [ptr, len] of texts) exports.torque_lua_free(ptr, len);
  };
  // Runs `f` as a call of the game's whose requests `host` serves: a module that trapped (or threw) is broken from then on,
  // and every later call is refused with the status of a panic.
  const call = (host, f) => {
    if (broken) return PANIC;
    const handle = nextHandle;
    nextHandle = nextHandle >= 0xffffffff ? 1 : nextHandle + 1;
    hosts.set(handle, host);
    try {
      return f(handle);
    } catch (error) {
      broken = error;
      console.error('torque_lua.wasm stopped:', error);
      return PANIC;
    } finally {
      hosts.delete(handle);
    }
  };

  return {
    runFile(host, vm, side, path, source, pkg) {
      return call(host, (handle) => {
        // Copied first: the game's views of its own memory are valid only until it runs again.
        const texts = [put(path), put(source), put(pkg)];
        try {
          return exports.torque_lua_run_file(handle, BigInt(vm), side, ...texts.flat());
        } finally {
          free(texts);
        }
      });
    },
    callDefined(host, vm, side, id, args) {
      return call(host, (handle) => {
        const texts = args.map(put);
        const list = exports.torque_lua_alloc(8 * texts.length);
        const v = view();
        texts.forEach(([ptr, len], i) => {
          v.setUint32(list + 8 * i, ptr, true);
          v.setUint32(list + 8 * i + 4, len, true);
        });
        try {
          const status = exports.torque_lua_call_defined(handle, BigInt(vm), side, BigInt(id), list, texts.length);
          if (status !== OK) return status;
          return read(exports.torque_lua_result(), exports.torque_lua_result_len());
        } finally {
          free([...texts, [list, 8 * texts.length]]);
        }
      });
    },
    realmDropped(vm) {
      if (!broken) {
        try {
          exports.torque_lua_vm_dropped(BigInt(vm));
        } catch (error) {
          broken = error;
          console.error('torque_lua.wasm stopped:', error);
        }
      }
    },
    /** Whether the module trapped or threw (and runs nothing more). */
    get broken() {
      return broken !== null;
    },
  };
}
