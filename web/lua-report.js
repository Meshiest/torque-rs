// How the web checks of the Lua tests (lua-node.mjs, lua-check.mjs) print the cases' results.

/** Prints `results` (web/lua-cases.js's) as `where` ran them; true when every case passed. */
export function report(where, results) {
  const failed = results.filter((r) => !r.ok);
  for (const r of results) {
    const time = r.seconds === undefined ? '' : ` (${r.seconds.toFixed(2)} s)`;
    const parity = r.parity ? ` parity ${r.parity}` : '';
    console.log(`${where}: ${r.ok ? 'ok' : 'FAILED'} ${r.name}${time}${parity}${r.afterFailure ? ' (after a failure)' : ''}`);
    if (!r.ok) console.log(`  ${String(r.error).replaceAll('\n', '\n  ')}`);
  }
  console.log(`${where}: ${results.length - failed.length} of ${results.length} cases passed`);
  return failed.length === 0 && results.length > 0;
}
