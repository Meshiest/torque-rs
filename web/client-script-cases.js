// Runs the client-script host's cases built for the web (tests/web/client-script-check, the cases tests/cases.rs runs
// under Wasmtime) in the folder `node web/lua-build.mjs --client --check` makes: under Node (client-script-node.mjs) and
// in a page (client-script-check.html, which web/client-script-check.mjs loads in headless Chrome). Each realm's guest runs
// in a Worker of client-script.js; a case waits for its Workers between steps. A failed case may trap the cases' module
// (a panic aborts on the web), so the cases after one that failed are marked as run after a failure.
import init, { cases, Run } from './check.js';
import { clientScripts } from './client-script.js';

/** Runs every case with the cases' module `check` and the realm guest's bytes `guest`: `[{ name, ok, seconds, error }]`. */
export async function runCases({ check, guest }) {
  await init({ module_or_path: check });
  const scripts = await clientScripts({ guest, unchecked: true });
  globalThis.torqueClientScript = scripts;
  const results = [];
  let failed = false;
  for (const name of cases()) {
    const started = performance.now();
    const seconds = () => (performance.now() - started) / 1000;
    const run = Run.of(name);
    try {
      let state;
      while ((state = run.step()) === 0) await scripts.settled();
      if (state === 2) throw new Error(run.error());
      results.push({ name, ok: true, seconds: seconds(), afterFailure: failed });
    } catch (error) {
      failed = true;
      results.push({ name, ok: false, seconds: seconds(), error: String(error) });
    } finally {
      // Its realms' Workers end with it.
      run?.free();
    }
  }
  if (scripts.live() !== 0) results.push({ name: 'no Worker is left', ok: false, error: `${scripts.live()} left` });
  return results;
}
