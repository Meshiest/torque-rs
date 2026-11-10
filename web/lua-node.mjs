// The Lua extension's tests on the web, under Node: run from the folder `node web/lua-build.mjs --check` makes (it copies
// this file there, beside check.js, lua.js, lua-cases.js, lua-report.js and the two modules). Exits 1 unless every case passed.
//
// Usage: node <target>/lua-web/check/lua-node.mjs
import { readFileSync } from 'node:fs';
import { runCases } from './lua-cases.js';
import { report } from './lua-report.js';

const compile = (file) => WebAssembly.compile(readFileSync(new URL(file, import.meta.url)));
const [lua, check] = await Promise.all([compile('torque_lua.wasm'), compile('check_bg.wasm')]);
const ok = report(`node ${process.version}`, await runCases({ lua, check }));
process.exit(ok ? 0 : 1);
