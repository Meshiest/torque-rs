// Builds the Lua extension (crates/torque-plugin-lua) as the browser build's WebAssembly module,
// <target>/lua-web/torque_lua.wasm: the same Rust and the same Luau (mlua's vendored sources) as the native library, compiled
// for wasm32-wasip1 with the WASI SDK's clang. Luau's errors are C++ exceptions, built for WebAssembly's exception handling
// (the standard `exnref` form, the one the SDK's C++ runtime is built for); the page's shim (web/lua.js) supplies the few
// WASI imports the module has.
//
// Usage: node lua-build.mjs [--check]
// With `--check` it also builds the Lua tests for the web (examples/lua-web-check, the engine with the page's Lua module) and
// puts them, wasm-bindgen's glue, web/lua.js and the module into <target>/lua-web/check/, for web/lua-check.mjs to run.
// wasm-bindgen's command line, at the version Cargo.lock names, is installed into <target>/web-tools at first.
// Needs the wasm32-wasip1 target (`rustup target add wasm32-wasip1`) and the WASI SDK, unpacked anywhere, named by WASI_SDK_PATH
// (its folder with `bin/` and `share/wasi-sysroot/`). The build folder is CARGO_TARGET_DIR's, else `target`. Prints the module's
// size, raw and gzipped.
//
// mlua 0.12.2 does not build for WASI as released (its heap dump names `libc::tmpfile`); until a release has upstream's fix
// (mlua commit 26c52c6c, which leaves the heap dump empty on Wasm), this copies mlua's registry sources into the build folder,
// applies that one change, and builds a copy of the extension's manifest that patches mlua with them. The repository's own
// manifest and lockfile are never touched. A release with the fix is used as it is.
import { spawnSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gzipSync } from 'node:zlib';

const flags = process.argv.slice(2);
const check = flags.includes('--check');
if (flags.some((f) => f !== '--check')) {
  console.error('usage: node lua-build.mjs [--check]');
  process.exit(2);
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const crate = join(root, 'crates', 'torque-plugin-lua');
const target = resolve(process.env.CARGO_TARGET_DIR ?? join(root, 'target'));
const work = join(target, 'lua-web');
const suffix = extname(process.execPath);
const slashes = (path) => path.replaceAll('\\', '/');

const sdk = process.env.WASI_SDK_PATH;
if (!sdk) {
  console.error(
    'WASI_SDK_PATH is not set: unpack a WASI SDK release (github.com/WebAssembly/wasi-sdk/releases) and set WASI_SDK_PATH to ' +
      'its folder (the one with bin/ and share/wasi-sysroot/)',
  );
  process.exit(2);
}
const clang = join(sdk, 'bin', `clang${suffix}`);
const libs = join(sdk, 'share', 'wasi-sysroot', 'lib', 'wasm32-wasip1');
if (!existsSync(clang) || !existsSync(join(libs, 'eh', 'libunwind.a'))) {
  console.error(`WASI_SDK_PATH (${sdk}) has no bin/clang or no C++ runtime with exceptions (share/wasi-sysroot/lib/wasm32-wasip1/eh)`);
  process.exit(2);
}

function run(cmd, args, options = {}) {
  console.log(`> ${cmd} ${args.join(' ')}`);
  const r = spawnSync(cmd, args, { cwd: root, stdio: options.capture ? ['ignore', 'pipe', 'inherit'] : 'inherit', encoding: 'utf8', maxBuffer: 1 << 26, env: { ...process.env, ...options.env } });
  if (r.status !== 0) {
    console.error(`${cmd} failed with ${r.status}`);
    process.exit(r.status ?? 1);
  }
  return r.stdout;
}

const manifest = join(crate, 'Cargo.toml');
const metadata = JSON.parse(
  run('cargo', ['metadata', '--format-version', '1', '--locked', '--manifest-path', manifest, '--filter-platform', 'wasm32-wasip1'], { capture: true }),
);
const mlua = metadata.packages.find((p) => p.name === 'mlua');
if (!mlua) {
  console.error('the extension depends on no mlua');
  process.exit(1);
}
const mluaDir = dirname(mlua.manifest_path);
const heapDump = join('src', 'luau', 'heap_dump.rs');
const WASM = 'any(all(target_arch = "wasm32", target_os = "unknown"), target_os = "wasi")';
const fixed = readFileSync(join(mluaDir, heapDump), 'utf8').includes('target_os = "wasi"');

mkdirSync(work, { recursive: true });
let buildManifest = manifest;
if (!fixed) {
  // mlua's sources with upstream's change.
  const patched = join(work, `mlua-${mlua.version}`);
  rmSync(patched, { recursive: true, force: true });
  cpSync(mluaDir, patched, { recursive: true });
  const file = join(patched, heapDump);
  const source = readFileSync(file, 'utf8').replaceAll('\r\n', '\n');
  const before = '        let mut buf = Vec::new();\n        unsafe {\n            let file = libc::tmpfile();';
  const after =
    `        let mut buf = Vec::new();\n        #[cfg(${WASM})]\n        let _ = (state, &mut buf, category_name);\n` +
    `        #[cfg(not(${WASM}))]\n        unsafe {\n            let file = libc::tmpfile();`;
  if (source.split(before).length !== 2) {
    console.error(`${file}: the heap dump is not the one mlua ${mlua.version} has; update web/lua-build.mjs`);
    process.exit(1);
  }
  writeFileSync(file, source.replace(before, after));

  // The extension's manifest and lockfile, its paths made absolute and mlua patched.
  const copy = join(work, 'crate');
  mkdirSync(copy, { recursive: true });
  const crates = slashes(join(root, 'crates'));
  let toml = readFileSync(manifest, 'utf8')
    .replaceAll('\r\n', '\n')
    .replaceAll('path = "../', `path = "${crates}/`)
    .replace('[lib]\nname = "torque_lua"\n', `[lib]\nname = "torque_lua"\npath = "${slashes(join(crate, 'src', 'lib.rs'))}"\n`);
  if (!toml.includes(`path = "${slashes(join(crate, 'src', 'lib.rs'))}"`)) {
    console.error(`${manifest}: no [lib] named torque_lua; update web/lua-build.mjs`);
    process.exit(1);
  }
  toml += `\n[patch.crates-io]\nmlua = { path = "${slashes(patched)}" }\n`;
  writeFileSync(join(copy, 'Cargo.toml'), toml);
  writeFileSync(join(copy, 'Cargo.lock'), readFileSync(join(crate, 'Cargo.lock')));
  buildManifest = join(copy, 'Cargo.toml');
}

// The SDK's compilers for Luau with its C++ runtime built for exceptions; the extension is built as a static library and
// linked by the SDK's clang against the SDK's C library, as a reactor module (its `_initialize` runs the constructors).
const cxxflags = '-fexceptions -fwasm-exceptions -mllvm -wasm-use-legacy-eh=false';
const env = {
  CC_wasm32_wasip1: clang,
  CXX_wasm32_wasip1: join(sdk, 'bin', `clang++${suffix}`),
  AR_wasm32_wasip1: join(sdk, 'bin', `llvm-ar${suffix}`),
  CXXSTDLIB_wasm32_wasip1: 'c++',
  LUAU_CXXFLAGS: cxxflags,
  CARGO_TARGET_DIR: target,
  // A rustc wrapper (sccache) is turned off, as web/build.mjs does.
  RUSTC_WRAPPER: '',
};
run(
  'cargo',
  ['rustc', '--config', "build.rustc-wrapper=''", '--lib', '--crate-type', 'staticlib', '--profile', 'web-lua',
    '--target', 'wasm32-wasip1', '--manifest-path', buildManifest],
  { env },
);
const library = join(target, 'wasm32-wasip1', 'web-lua', 'libtorque_lua.a');
const EXPORTS = [
  'torque_lua_start', 'torque_lua_alloc', 'torque_lua_free', 'torque_lua_run_file', 'torque_lua_call_defined',
  'torque_lua_result', 'torque_lua_result_len', 'torque_lua_vm_dropped',
];
const built = join(work, 'torque_lua.wasm');
run(clang, [
  '--target=wasm32-wasip1', '-mexec-model=reactor', '-fwasm-exceptions', '-o', built, library,
  `-L${join(libs, 'eh')}`, `-L${libs}`, '-lc++', '-lc++abi', '-lunwind',
  // Luau's `os.clock` is C's `clock()`, which WASI's C library has only as an emulation over its CPU-time clock.
  '-lwasi-emulated-process-clocks',
  // Room for Lua and TorqueScript calling each other deeply (the engine's call depth stops them first).
  '-Wl,-z,stack-size=8388608',
  '-Wl,--strip-all',
  ...EXPORTS.map((name) => `-Wl,--export=${name}`),
]);

const out = built;
const bytes = readFileSync(built);
try {
  await WebAssembly.compile(bytes);
} catch (error) {
  console.error(`${built}: not a module this Node compiles (it needs WebAssembly exception handling): ${error.message}`);
  process.exit(1);
}
const kib = (n) => `${(n / 1024).toFixed(0)} KiB`;
console.log(`${out}: ${kib(statSync(out).size)} (${kib(gzipSync(bytes, { level: 9 }).length)} gzipped)`);

if (check) {
  // The cases, in the release profile (the step-limit cases run millions of steps), and wasm-bindgen's glue for a page.
  run(
    'cargo',
    ['build', '--config', "build.rustc-wrapper=''", '--release', '--target', 'wasm32-unknown-unknown', '-p', 'torque-example-lua-web-check'],
    { env: { CARGO_TARGET_DIR: target, RUSTC_WRAPPER: '' } },
  );
  const lock = readFileSync(join(root, 'Cargo.lock'), 'utf8');
  const version = /\nname = "wasm-bindgen"\nversion = "([^"]+)"/.exec(lock)?.[1];
  if (!version) {
    console.error('Cargo.lock names no wasm-bindgen');
    process.exit(1);
  }
  const tools = join(target, 'web-tools');
  const bindgen = join(tools, 'bin', `wasm-bindgen${suffix}`);
  const have = existsSync(bindgen) ? (spawnSync(bindgen, ['--version'], { encoding: 'utf8' }).stdout ?? '') : '';
  if (!have.includes(version)) run('cargo', ['install', 'wasm-bindgen-cli', '--version', version, '--locked', '--root', tools]);
  const folder = join(work, 'check');
  rmSync(folder, { recursive: true, force: true });
  mkdirSync(folder, { recursive: true });
  const cases = join(target, 'wasm32-unknown-unknown', 'release', 'torque_example_lua_web_check.wasm');
  run(bindgen, ['--target', 'web', '--no-typescript', '--out-dir', folder, '--out-name', 'check', cases]);
  for (const page of ['lua.js', 'lua-cases.js', 'lua-report.js', 'lua-node.mjs', 'lua-check.html']) cpSync(join(root, 'web', page), join(folder, page));
  cpSync(out, join(folder, 'torque_lua.wasm'));
  console.log(`${folder}: the Lua tests for the web`);
}
