// Builds the Lua extension (crates/torque-plugin-lua) as the browser build's WebAssembly module,
// <target>/lua-web/torque_lua.wasm: the same Rust and the same Luau (mlua's vendored sources) as the native library, compiled
// for wasm32-wasip1 with the WASI SDK's clang. Luau's errors are C++ exceptions, built for WebAssembly's exception handling
// (the standard `exnref` form, the one the SDK's C++ runtime is built for); the page's shim (web/lua.js) supplies the few
// WASI imports the module has.
//
// Usage: node lua-build.mjs [--check] [--client]
// With `--client` it builds the client-script guest instead, <target>/lua-web/torque_lua_client.wasm: the same crate with its
// `client-script` feature, whose module has the realm's entry points (`torque_realm_*`, crates/torque-client-script-format)
// and none of the extension's.
// With `--check` it also builds the Lua tests for the web (tests/web/lua-check, the engine with the page's Lua module) and
// puts them, wasm-bindgen's glue, web/lua.js and the module into <target>/lua-web/check/, for web/lua-check.mjs to run. With
// both, it builds the client-script host's cases instead (tests/web/client-script-check) and puts them, the glue,
// web/client-script*.js and the guest into <target>/lua-web/client-check/, for web/client-script-check.mjs.
// wasm-bindgen's command line, at the version Cargo.lock names, is installed into <target>/web-tools at first.
// Needs the wasm32-wasip1 target (`rustup target add wasm32-wasip1`) and the WASI SDK, unpacked anywhere, named by WASI_SDK_PATH
// (its folder with `bin/` and `share/wasi-sysroot/`). The build folder is CARGO_TARGET_DIR's, else `target`. Prints the module's
// size, raw and gzipped.
//
// The crates are built from a copy of them, laid out as in the checkout, in the build folder (<target>/lua-web/repo): one workspace
// whose paths are all relative to its root, so the module does not depend on where the checkout or the build folder are (cargo
// hashes the place of a path dependency outside its workspace into the symbol names). Besides that, the module's bytes are a
// function of the sources, the lockfile, the Rust toolchain and the WASI SDK. The repository's own manifests and lockfiles are
// never touched.
//
// mlua 0.12.2 does not build for WASI as released (its heap dump names `libc::tmpfile`); until a release has upstream's fix
// (mlua commit 26c52c6c, which leaves the heap dump empty on Wasm), this puts a copy of mlua's registry sources with that one
// change into the copy and patches mlua with them. A release with the fix is used as it is.
import { spawnSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, extname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gzipSync } from 'node:zlib';

const flags = process.argv.slice(2);
const check = flags.includes('--check');
const client = flags.includes('--client');
if (flags.some((f) => f !== '--check' && f !== '--client')) {
  console.error('usage: node lua-build.mjs [--check] [--client]');
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

// The crates the extension is built from, copied into one workspace of their own at <build folder>/lua-web/repo, laid out as in
// the checkout. Cargo hashes a path dependency into its crates' symbol names by its place in the workspace when it is inside
// it and by its absolute path when it is outside, so with the extension a workspace of its own and its dependencies beside it,
// the checkout's folder would be in the module. In this copy every path is relative to the workspace's root and the module is
// the same wherever the checkout and the build folder are. The repository's own manifests and lockfiles are never touched.
const tree = join(work, 'repo');
rmSync(tree, { recursive: true, force: true });
mkdirSync(tree, { recursive: true });
// Every crate of the lockfile, those another platform's dependencies name included: cargo reads their manifests too.
const everything = JSON.parse(run('cargo', ['metadata', '--format-version', '1', '--locked', '--manifest-path', manifest], { capture: true }));
const locals = everything.packages.filter((p) => p.source === null);
const skip = (path) => !/[\\/](target|\.git|\.scratch)$/.test(path);
const read = (file) => readFileSync(file, 'utf8').replaceAll('\r\n', '\n');
// A member's dev-dependencies are part of its workspace's resolution, which the extension's lockfile (made with its crates as plain
// dependencies, whose dev-dependencies are never read) does not have: the copies have none, and nothing here is tested.
function withoutDevDependencies(text) {
  let skipping = false;
  return text
    .split('\n')
    .filter((line) => {
      if (line.startsWith('[')) skipping = /^\[(target\.[^\]]*\.)?dev-dependencies([.\]])/.test(line);
      return !skipping;
    })
    .join('\n');
}
const members = locals.map((p) => {
  const from = dirname(p.manifest_path);
  const rel = slashes(relative(root, from));
  if (rel.startsWith('..')) {
    console.error(`${from}: a crate of the extension outside the checkout`);
    process.exit(1);
  }
  cpSync(from, join(tree, rel), { recursive: true, filter: skip });
  const copied = join(tree, rel, 'Cargo.toml');
  writeFileSync(copied, withoutDevDependencies(read(copied)));
  return rel;
});
// Cargo reads the manifest of every path dependency a member names, the optional ones nobody turns on too. Those that are not in the
// lockfile are stood in for by a package of the same name with nothing in it, which is never built.
const have = new Set(locals.map((p) => resolve(dirname(p.manifest_path))));
for (const p of locals) {
  const dir = dirname(p.manifest_path);
  for (const [, to] of withoutDevDependencies(read(p.manifest_path)).matchAll(/path\s*=\s*"([^"]+)"/g)) {
    const dep = resolve(dir, to);
    if (have.has(dep) || !existsSync(join(dep, 'Cargo.toml'))) continue;
    have.add(dep);
    const name = /^name\s*=\s*"([^"]+)"/m.exec(read(join(dep, 'Cargo.toml')))?.[1];
    const stub = join(tree, slashes(relative(root, dep)));
    mkdirSync(stub, { recursive: true });
    writeFileSync(join(stub, 'Cargo.toml'), `[package]\nname = "${name}"\nversion = "0.1.0"\nedition = "2024"\n\n[lib]\npath = "lib.rs"\n`);
    writeFileSync(join(stub, 'lib.rs'), '');
  }
}
const extension = relative(root, crate).replaceAll('\\', '/');
if (!members.includes(extension)) {
  console.error(`${manifest}: not among its own packages`);
  process.exit(1);
}
// The checkout's workspace manifest (the crates' shared package fields, dependencies and lints) with these crates as its members,
// and the extension's profiles; the extension's own manifest loses the workspace it declared.
const lua = read(manifest);
const profileAt = lua.indexOf('[profile.web-lua]');
if (profileAt < 0 || !lua.includes('\n[workspace]\n')) {
  console.error(`${manifest}: no [workspace] or no [profile.web-lua]; update web/lua-build.mjs`);
  process.exit(1);
}
let toml = read(join(root, 'Cargo.toml'));
const list = /members = \[[^\]]*\]/;
const skipped = /exclude = \[[^\]]*\]/;
if (!list.test(toml) || !skipped.test(toml)) {
  console.error(`${join(root, 'Cargo.toml')}: no members and exclude lists; update web/lua-build.mjs`);
  process.exit(1);
}
toml = toml
  .replace(list, `members = [${members.map((m) => `"${m}"`).join(', ')}]`)
  .replace(skipped, 'exclude = []');
toml += `\n${lua.slice(profileAt)}`;
writeFileSync(join(tree, 'Cargo.lock'), readFileSync(join(crate, 'Cargo.lock')));
writeFileSync(join(tree, extension, 'Cargo.toml'), withoutDevDependencies(lua).replace('\n[workspace]\n', '\n'));
if (!fixed) {
  // mlua's sources with upstream's change.
  const patched = join(tree, 'vendor', `mlua-${mlua.version}`);
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
  toml += `\n[patch.crates-io]\nmlua = { path = "vendor/mlua-${mlua.version}" }\n`;
}
writeFileSync(join(tree, 'Cargo.toml'), toml);
const buildManifest = join(tree, extension, 'Cargo.toml');

// Where the build reads from, named by the same stand-ins wherever it runs: a panic's file name and a C++ assertion's are in
// the module, so without this its bytes (and the hash the engine accepts it by) would depend on the checkout's folder, the
// build folder, the cargo home and the toolchain's folder. A path later in the list wins where two overlap (the build
// folder may sit in the checkout).
const sysroot = spawnSync('rustc', ['--print', 'sysroot'], { encoding: 'utf8' }).stdout?.trim();
const cargoHome = resolve(process.env.CARGO_HOME ?? join(homedir(), '.cargo'));
// Each folder as the compiler sees it: through its symbolic links (macOS's /var is /private/var), or a prefix would not match.
mkdirSync(target, { recursive: true });
const real = (path) => realpathSync(path);
const stand = [[real(root), '/torque-rs'], [real(cargoHome), '/cargo'], ...(sysroot ? [[real(sysroot), '/rustup']] : []), [real(target), '/build']];
// Luau's C++ flags are split on whitespace, so a folder with a space in it cannot be named to the C++ compiler: refused, not skipped.
for (const [from] of stand) {
  if (/\s/.test(from)) {
    console.error(`${from}: a folder the build reads from has whitespace in its name, which the C++ path remap cannot name; build from a folder without`);
    process.exit(2);
  }
}
const rustflags = stand.map(([from, to]) => `--remap-path-prefix=${from}=${to}`).join('');
// The SDK's compilers for Luau with its C++ runtime built for exceptions; the extension is built as a static library and
// linked by the SDK's clang against the SDK's C library, as a reactor module (its `_initialize` runs the constructors).
const cxxflags = ['-fexceptions', '-fwasm-exceptions', '-mllvm', '-wasm-use-legacy-eh=false', ...stand.map(([from, to]) => `-ffile-prefix-map=${slashes(from)}=${to}`)].join(' ');
const env = {
  CARGO_ENCODED_RUSTFLAGS: rustflags,
  CC_wasm32_wasip1: clang,
  CXX_wasm32_wasip1: join(sdk, 'bin', `clang++${suffix}`),
  AR_wasm32_wasip1: join(sdk, 'bin', `llvm-ar${suffix}`),
  CXXSTDLIB_wasm32_wasip1: 'c++',
  LUAU_CXXFLAGS: cxxflags,
  CARGO_TARGET_DIR: target,
  // A rustc wrapper (sccache) is turned off, as web/build.mjs does.
  RUSTC_WRAPPER: '',
};
const profile = client ? 'web-lua-client' : 'web-lua';
run(
  'cargo',
  ['rustc', '--config', "build.rustc-wrapper=''", '--lib', '--crate-type', 'staticlib', '--profile', profile,
    '--target', 'wasm32-wasip1', '--manifest-path', buildManifest, ...(client ? ['--features', 'client-script'] : [])],
  { env },
);
const library = join(target, 'wasm32-wasip1', profile, 'libtorque_lua.a');
const EXPORTS = client
  ? [
      'torque_realm_format', 'torque_realm_tiers', 'torque_realm_alloc', 'torque_realm_free', 'torque_realm_new',
      'torque_realm_add_script', 'torque_realm_run', 'torque_realm_frame', 'torque_realm_ops', 'torque_realm_ops_len',
      'torque_realm_pending',
    ]
  : [
      'torque_lua_start', 'torque_lua_alloc', 'torque_lua_free', 'torque_lua_run_file', 'torque_lua_call_defined',
      'torque_lua_result', 'torque_lua_result_len', 'torque_lua_vm_dropped',
    ];
const built = join(work, client ? 'torque_lua_client.wasm' : 'torque_lua.wasm');
run(clang, [
  '--target=wasm32-wasip1', '-mexec-model=reactor', '-fwasm-exceptions', '-o', built, library,
  `-L${join(libs, 'eh')}`, `-L${libs}`, '-lc++', '-lc++abi', '-lunwind',
  // Luau's `os.clock` is C's `clock()`, which WASI's C library has only as an emulation over its CPU-time clock.
  '-lwasi-emulated-process-clocks',
  // Room for Lua and TorqueScript calling each other deeply (the engine's call depth stops them first); the client
  // guest's Lua nests within Luau's own C-call limit.
  '-Wl,-z,stack-size=8388608',
  '-Wl,--strip-all',
  ...EXPORTS.map((name) => `-Wl,--export=${name}`),
]);

const out = built;
const bytes = readFileSync(built);
{
  // Nothing of the machine the module was built on may be in it: the home folder, the user folders' name, the checkout, the build
  // folder, the cargo home and the toolchain, written either way round and as the compiler saw them.
  const names = new Set(['Users']);
  for (const path of [homedir(), root, target, cargoHome, sysroot, ...stand.map(([from]) => from)]) {
    if (!path) continue;
    for (const form of [path, resolve(path), slashes(path), path.replaceAll('/', '\\')]) names.add(form);
  }
  const found = [...names].filter((name) => bytes.includes(Buffer.from(name)));
  if (found.length) {
    console.error(`${built}: names of the build machine are in the module: ${found.join(', ')}`);
    process.exit(1);
  }
  console.log(`no folder of the build machine is in the module (checked ${names.size} spellings)`);
}
try {
  const module = await WebAssembly.compile(bytes);
  if (client) {
    // The client guest reaches nothing of the engine: its imports are WASI's alone.
    const imports = WebAssembly.Module.imports(module);
    const foreign = imports.filter((i) => i.module !== 'wasi_snapshot_preview1');
    if (foreign.length) {
      console.error(`${built}: imports outside WASI: ${foreign.map((i) => `${i.module}.${i.name}`).join(', ')}`);
      process.exit(1);
    }
    console.log(`imports: ${imports.map((i) => i.name).join(', ')}`);
  }
} catch (error) {
  console.error(`${built}: not a module this Node compiles (it needs WebAssembly exception handling): ${error.message}`);
  process.exit(1);
}
const kib = (n) => `${(n / 1024).toFixed(0)} KiB`;
console.log(`${out}: ${kib(statSync(out).size)} (${kib(gzipSync(bytes, { level: 9 }).length)} gzipped)`);

if (check) {
  // The cases, optimized (the step-limit cases run millions of steps), and wasm-bindgen's glue for a page. The client
  // cases grant unreviewed tiers, which only a build with debug assertions may: their profile keeps them on.
  const example = client ? 'torque-test-client-script-web-check' : 'torque-test-lua-web-check';
  const casesProfile = client ? 'client-script-check' : 'release';
  run(
    'cargo',
    ['build', '--config', "build.rustc-wrapper=''", '--profile', casesProfile, '--target', 'wasm32-unknown-unknown', '-p', example],
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
  const folder = join(work, client ? 'client-check' : 'check');
  rmSync(folder, { recursive: true, force: true });
  mkdirSync(folder, { recursive: true });
  const cases = join(target, 'wasm32-unknown-unknown', casesProfile, `${example.replaceAll('-', '_')}.wasm`);
  run(bindgen, ['--target', 'web', '--no-typescript', '--out-dir', folder, '--out-name', 'check', cases]);
  const pages = client
    ? ['client-script.js', 'client-script-worker.js', 'client-script-cases.js', 'client-script-node.mjs', 'client-script-check.html']
    : ['lua.js', 'lua-cases.js', 'lua-node.mjs', 'lua-check.html'];
  for (const page of [...pages, 'lua-report.js']) cpSync(join(root, 'web', page), join(folder, page));
  cpSync(out, join(folder, client ? 'torque_lua_client.wasm' : 'torque_lua.wasm'));
  // Node takes the `.js` files (the Worker's among them) for ES modules, as a browser does.
  if (client) writeFileSync(join(folder, 'package.json'), '{ "type": "module" }\n');
  console.log(`${folder}: the Lua tests for the web`);
}
