// The client-script host's cases on the web, under Node: run from the folder `node web/lua-build.mjs --client --check`
// makes (it copies this file there, beside check.js, client-script*.js, lua-report.js and the two modules). Exits 1 unless
// every case passed.
//
// Usage: node <target>/lua-web/client-check/client-script-node.mjs
import { readFileSync } from 'node:fs';
import { runCases } from './client-script-cases.js';
import { report } from './lua-report.js';

const read = (file) => readFileSync(new URL(file, import.meta.url));
const check = await WebAssembly.compile(read('check_bg.wasm'));
const ok = report(`node ${process.version}`, await runCases({ check, guest: read('torque_lua_client.wasm') }));
process.exit(ok ? 0 : 1);
