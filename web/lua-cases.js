// Runs the Lua extension's test cases built for the web (examples/lua-web-check, the same cases as
// crates/torque-plugin-lua/tests/lua.rs), in the folder `node web/lua-build.mjs --check` makes: under Node (lua-node.mjs) and
// in a page (lua-check.html, which web/lua-check.mjs loads in headless Chrome).
//
// The engine's module is instantiated once, as the native tests share one process; each case gets a fresh instance of the
// Lua module, as each native test links the extension again. A failed case traps the engine's module (a panic aborts on the
// web), so the cases after one that failed are marked as run after a failure.
import init, { cases, runCase } from './check.js';
import { createLua } from './lua.js';

/** Runs every case with the compiled Lua module `lua` and engine module `check`: `[{ name, ok, seconds, error }]`. */
export async function runCases({ lua, check }) {
  await init({ module_or_path: check });
  const results = [];
  let failed = false;
  for (const name of cases()) {
    globalThis.torqueLua = await createLua(lua);
    const errors = [];
    const consoleError = console.error;
    console.error = (...args) => errors.push(args.map(String).join(' '));
    const started = performance.now();
    const seconds = () => (performance.now() - started) / 1000;
    try {
      if (!runCase(name)) throw new Error('no such case');
      results.push({ name, ok: true, seconds: seconds(), afterFailure: failed });
    } catch (error) {
      failed = true;
      results.push({ name, ok: false, seconds: seconds(), error: [...errors, String(error)].join('\n') });
    } finally {
      console.error = consoleError;
      delete globalThis.torqueLua;
    }
  }
  return results;
}
