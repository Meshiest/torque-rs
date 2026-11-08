// Runs a wasm32-wasip1 command module under Node's WASI and exits with its status: the headless wasm runner of the trace-hash
// check (`just trace-hash`, CI), which needs no browser. The module's standard output and error are Node's own; its arguments
// follow the module's path. No folder is given to the module (nothing is preopened), so it cannot read or write a file.
// Usage: node --no-warnings wasi-run.mjs <module.wasm> [arguments...]
import { readFileSync } from 'node:fs';
import { WASI } from 'node:wasi';

const [path, ...args] = process.argv.slice(2);
if (path === undefined) {
  console.error('usage: node wasi-run.mjs <module.wasm> [arguments...]');
  process.exit(2);
}
const wasi = new WASI({ version: 'preview1', args: [path, ...args], env: {}, returnOnExit: true });
const module = await WebAssembly.compile(readFileSync(path));
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
process.exit(wasi.start(instance));
