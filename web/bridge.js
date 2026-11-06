// The page's `torque` object: the page and the game's scripts calling each other (torque-app's `page_bridge`). No page
// needed: the tests run it in Node (`node --test web/bridge.test.mjs`).
//
// Page to script, each a promise of the result string, served at the game's next frame (requests made before the game runs
// wait for it):
//   torque.call('fnName', 'a', 'b')        a script function's result
//   torque.eval('return 1 + 2;')           script text's result; a syntax error rejects with `kind: 'syntax'`, `line`
//                                          and `column`, and nothing of the text runs
//   torque.getVar('$pref::Name'), torque.setVar('$pref::Name', 'Ann')
// A refusal or a failure rejects with a `TorqueError` (`kind`, `message`): arguments are strings, at most 16 of 4096 bytes
// each; names are identifiers.
//
// Script to page: `torque.on('mapLoaded', (name) => ...)` registers a handler, which script calls with
// `callPage("mapLoaded", %name)`; what the handler returns is the call's result (a string; handlers run synchronously).
// Script reaches only the names registered here, at most 32 calls a frame, and has no other way into the page. A remote
// server reaches the page only through the game's own `clientCmd` functions that call `callPage` with a registered name.

// An error the bridge gave: `kind` (`syntax`, `too-many-args`, `too-long`, `bad-name`, `no-function`, `queue-full`, `failed`,
// `no-bridge`), and for a syntax error `line` and `column`.
export class TorqueError extends Error {
  constructor(info) {
    super(info.message ?? String(info.kind));
    this.name = 'TorqueError';
    this.kind = info.kind;
    if (info.line !== undefined) this.line = info.line;
    if (info.column !== undefined) this.column = info.column;
  }
}

// The error the game rejected with (its JSON), or one made of anything else thrown.
export function decodeError(e) {
  try {
    const info = JSON.parse(String(e));
    if (info && typeof info.kind === 'string') return new TorqueError(info);
  } catch {}
  return new TorqueError({ kind: 'failed', message: String(e?.message ?? e) });
}

// The game's rule for a handler name (torque-app's `is_name`): an ASCII identifier of at most 64 bytes.
const NAME = /^[A-Za-z_][A-Za-z0-9_]{0,63}$/;
// The most handlers (as the game's `MAX_HANDLERS`) and the most calls queued before the game is attached.
const MAX_HANDLERS = 64;
const MAX_WAITING = 256;

// A `torque` object; `attach(module)` connects it to the game's module once it is loaded (until then requests wait).
export function createTorque() {
  let game = null;
  const waiting = [];
  const handlers = new Map();

  function send(kind, a, b, args) {
    return new Promise((resolve, reject) => {
      if (!game && waiting.length >= MAX_WAITING) {
        reject(new TorqueError({ kind: 'queue-full', message: `more than ${MAX_WAITING} calls wait for the game to start` }));
        return;
      }
      const go = () => {
        if (!game.torqueRequest) {
          reject(new TorqueError({ kind: 'no-bridge', message: "this game's page has no bridge to its scripts" }));
          return;
        }
        game.torqueRequest(kind, a, b, args, resolve, (e) => reject(decodeError(e)));
      };
      if (game) go();
      else waiting.push(go);
    });
  }

  const register = (name) => {
    try {
      game?.torqueOn?.(name);
    } catch (e) {
      handlers.delete(name);
      throw decodeError(e);
    }
  };

  return {
    call: (fn, ...args) => send('call', String(fn), '', args.map(String)),
    eval: (text) => send('eval', String(text), '', []),
    getVar: (name) => send('get', String(name), '', []),
    setVar: (name, value) => send('set', String(name), String(value), []),
    on(name, fn) {
      if (typeof fn !== 'function') throw new TypeError(`torque.on(${name}): a handler is a function`);
      name = String(name);
      if (!NAME.test(name)) {
        throw new TorqueError({ kind: 'bad-name', message: `${JSON.stringify(name)} is not a name (an identifier of at most 64 bytes)` });
      }
      if (!handlers.has(name) && handlers.size >= MAX_HANDLERS) {
        throw new TorqueError({ kind: 'failed', message: `at most ${MAX_HANDLERS} handlers` });
      }
      handlers.set(name, fn);
      register(name);
    },
    off(name) {
      handlers.delete(String(name));
      game?.torqueOff?.(String(name));
    },
    // The game's `callPage`: the handler's result as a string ("" for none).
    dispatch(name, args) {
      const handler = handlers.get(name);
      if (!handler) throw new Error(`no handler ${name}`);
      const out = handler(...args);
      return out === undefined || out === null ? '' : String(out);
    },
    attach(module) {
      game = module;
      // Each name on its own: one the game refuses leaves the rest registered and the waiting calls still sent.
      for (const name of [...handlers.keys()]) {
        try {
          register(name);
        } catch {}
      }
      for (const go of waiting.splice(0)) go();
    },
  };
}
