// The determinism check: the trace-hash scenarios (crates/torque-trace-hash) run natively and as wasm32-wasip1, and the lines
// they print must be equal, and equal to the lines pinned in crates/torque-trace-hash/expected.txt. The native run is made twice (`--runs <n>`, default 2) and must repeat itself first. The wasm module
// is run by Node's WASI (web/wasi-run.mjs): no browser and no window.
//
// Usage: node trace-hash.mjs [--runs <n>] [--release]
// Needs the wasm32-wasip1 target (`rustup target add wasm32-wasip1`). The build folder is CARGO_TARGET_DIR's, else `target`.
// Prints each scenario's line once, and exits 1 when a run differs from the first native one.
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const args = process.argv.slice(2);
const release = args.includes('--release');
const runsAt = args.indexOf('--runs');
const runs = runsAt < 0 ? 2 : Number(args[runsAt + 1]);
const known = new Set(['--release', '--runs', String(runs)]);
if (!Number.isInteger(runs) || runs < 1 || args.some((a) => !known.has(a))) {
  console.error('usage: node trace-hash.mjs [--runs <n>] [--release]');
  process.exit(2);
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const target = resolve(process.env.CARGO_TARGET_DIR ?? join(root, 'target'));
const profile = release ? 'release' : 'debug';
const profileFlags = release ? ['--release'] : [];
// A rustc wrapper (sccache) is turned off for the wasm build, as web/build.mjs does.
const noWrapper = ['--config', "build.rustc-wrapper=''"];

function run(cmd, cmdArgs, env = {}) {
  console.log(`> ${cmd} ${cmdArgs.join(' ')}`);
  const r = spawnSync(cmd, cmdArgs, { cwd: root, encoding: 'utf8', env: { ...process.env, ...env }, maxBuffer: 1 << 24 });
  if (r.stderr) process.stderr.write(r.stderr);
  if (r.status !== 0) {
    console.error(`${cmd} failed with ${r.status}`);
    process.exit(r.status ?? 1);
  }
  return r.stdout;
}

const native = [];
for (let i = 0; i < runs; i++) {
  native.push(run('cargo', ['run', '--quiet', '-p', 'torque-trace-hash', ...profileFlags]));
}
const wasmFile = join(target, 'wasm32-wasip1', profile, 'trace-hash.wasm');
run('cargo', ['build', ...noWrapper, '--target', 'wasm32-wasip1', '-p', 'torque-trace-hash', ...profileFlags], { RUSTC_WRAPPER: '' });
const wasm = run(process.execPath, ['--no-warnings', join(root, 'web', 'wasi-run.mjs'), wasmFile]);

let ok = true;
native.forEach((out, i) => {
  if (out !== native[0]) {
    console.error(`native run ${i + 1} differs from run 1:\n${out}`);
    ok = false;
  }
});
if (wasm !== native[0]) {
  console.error(`wasm differs from native:\n--- native\n${native[0]}--- wasm\n${wasm}`);
  ok = false;
}
if (!/^all [a-z]+ [0-9a-f]{16}$/m.test(native[0]) || native[0].split('\n').filter(Boolean).length < 3) {
  console.error(`no scenarios ran:\n${native[0]}`);
  ok = false;
}
// The pinned lines (crates/torque-trace-hash/expected.txt, `#` lines are the reasons): a full run must print exactly these.
const pinned = readFileSync(join(root, 'crates', 'torque-trace-hash', 'expected.txt'), 'utf8')
  .split('\n')
  .map((l) => l.trim())
  .filter((l) => l && !l.startsWith('#'));
const printed = native[0].split('\n').map((l) => l.trim()).filter(Boolean);
const missing = pinned.filter((l) => !printed.includes(l));
const unpinned = printed.filter((l) => !pinned.includes(l));
if (missing.length || unpinned.length) {
  console.error(
    `the lines differ from expected.txt:\n--- pinned, not printed\n${missing.join('\n')}\n--- printed, not pinned\n${unpinned.join('\n')}`,
  );
  ok = false;
}
if (!ok) process.exit(1);
console.log(native[0].trimEnd());
console.log(`native (${runs} runs) = wasm32-wasip1`);
