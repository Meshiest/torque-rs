// Builds an example for the web into <target>/web/<name>/: the module for wasm32-unknown-unknown, wasm-bindgen's JavaScript
// glue, the page and its file loader, and the example's files, packed by the native build.
//
// Usage: node build.mjs <example> [--embed] [--solo]      (level-demo, fps, vehicle-demo; vehicle-demo fetches its files only)
// By default the files are fetched by the page: the folder holds `files.json` (each file's path, size and SHA-256) and
// `game.pack`, which the native build writes (`--web-files`), and the module carries none of them. With `--embed` the module
// carries the pack (the example's `embed-pack` feature; `--web-pack` writes it to <target>/web/<name>.pack) and the folder is
// <target>/web/<name>-embedded/.
// With `--solo` the page is marked to be played alone (its root element's `data-torque-solo`, for a site with no game server:
// the arena then plays on its own server in the page's worker, and no menu offers to host or join) and the folder is
// <target>/web/<name>-solo/ (`-embedded-solo` with `--embed`).
//
// The folder, served as it is: index.html, files.js, loading.js, settings.js, bridge.js, host.js and host-worker.js (a
// browser host), game.js, game_bg.wasm, and files.json with game.pack when fetched.
// The target folder is CARGO_TARGET_DIR's, else the workspace's `target`. wasm-bindgen's command line, at the version
// Cargo.lock names, is installed into <target>/web-tools at first.
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Each example's package and program, and whether it can carry its files (`--embed`, its `embed-pack` feature).
const EXAMPLES = {
  'level-demo': { package: 'torque-example-level-demo', bin: 'example-level-demo', embed: true },
  fps: { package: 'torque-example-fps', bin: 'fps-client', embed: true },
  'vehicle-demo': { package: 'torque-example-vehicle-demo', bin: 'vehicle-demo', embed: false },
};

const name = process.argv[2];
const example = EXAMPLES[name];
const flags = process.argv.slice(3);
const embed = flags.includes('--embed');
const solo = flags.includes('--solo');
if (!example || flags.some((a) => a !== '--embed' && a !== '--solo')) {
  console.error(`usage: node build.mjs <${Object.keys(EXAMPLES).join('|')}> [--embed] [--solo]`);
  process.exit(2);
}
if (embed && !example.embed) {
  console.error(`${name} has no build that carries its files: leave out --embed`);
  process.exit(2);
}
if (embed && solo && name === 'fps') {
  console.error('the arena played alone runs its server in a worker, which fetches the files: leave out --embed');
  process.exit(2);
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const target = resolve(process.env.CARGO_TARGET_DIR ?? join(root, 'target'));
const out = join(target, 'web', `${name}${embed ? '-embedded' : ''}${solo ? '-solo' : ''}`);
const pack = embed ? join(target, 'web', `${name}.pack`) : join(out, 'game.pack');
const tools = join(target, 'web-tools');
// A program's file name ends as Node's own does (nothing outside Windows).
const suffix = extname(process.execPath);

function run(cmd, args, env = {}) {
  console.log(`> ${cmd} ${args.join(' ')}`);
  const r = spawnSync(cmd, args, { cwd: root, stdio: 'inherit', env: { ...process.env, ...env } });
  if (r.status !== 0) process.exit(r.status ?? 1);
}

rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });

// The files, packed by the native build: into the folder with their list, or beside it to be embedded.
const native = ['run', '-p', example.package, '--release', '--bin', example.bin, '--'];
run('cargo', [...native, ...(embed ? ['--web-pack', pack] : ['--web-files', out])]);

// The module, in the `web` profile (release with thin LTO). A rustc wrapper (sccache) is turned off, set in the config or in
// RUSTC_WRAPPER: its command lines for web-sys are too long for Windows.
run(
  'cargo',
  ['build', '--config', "build.rustc-wrapper=''", '--profile', 'web', '--target', 'wasm32-unknown-unknown',
    '-p', example.package, '--bin', example.bin, ...(embed ? ['--features', 'embed-pack'] : [])],
  { RUSTC_WRAPPER: '', ...(embed ? { TORQUE_WEB_PACK: pack } : {}) },
);

// wasm-bindgen's command line at the version the module was built with.
const lock = readFileSync(join(root, 'Cargo.lock'), 'utf8');
const version = /\nname = "wasm-bindgen"\nversion = "([^"]+)"/.exec(lock)?.[1];
if (!version) {
  console.error('Cargo.lock names no wasm-bindgen');
  process.exit(1);
}
const bindgen = join(tools, 'bin', `wasm-bindgen${suffix}`);
const have = existsSync(bindgen) ? spawnSync(bindgen, ['--version'], { encoding: 'utf8' }).stdout ?? '' : '';
if (!have.includes(version)) {
  run('cargo', ['install', 'wasm-bindgen-cli', '--version', version, '--locked', '--root', tools]);
}

const wasm = join(target, 'wasm32-unknown-unknown', 'web', `${example.bin}.wasm`);
run(bindgen, ['--target', 'web', '--no-typescript', '--out-dir', out, '--out-name', 'game', wasm]);

// Binaryen's optimizer over the module when `wasm-opt` is on the PATH (or `WASM_OPT` names it; `WASM_OPT=0` skips it), on the
// features rustc's wasm32 target uses; on two cores. The result must still be a module Node's WebAssembly takes.
const moduleFile = join(out, 'game_bg.wasm');
const opt = process.env.WASM_OPT ?? `wasm-opt${suffix}`;
if (opt !== '0' && spawnSync(opt, ['--version'], { encoding: 'utf8' }).status === 0) {
  const features = ['bulk-memory', 'multivalue', 'mutable-globals', 'nontrapping-float-to-int', 'reference-types', 'sign-ext'];
  run(opt, ['-O2', ...features.map((f) => `--enable-${f}`), moduleFile, '-o', moduleFile], { BINARYEN_CORES: '2' });
  if (!WebAssembly.validate(readFileSync(moduleFile))) {
    console.error(`${moduleFile}: wasm-opt's output is not a valid module`);
    process.exit(1);
  }
} else {
  console.log('wasm-opt not found: the module is not optimized further (install binaryen for a faster one)');
}
for (const page of ['files.js', 'loading.js', 'settings.js', 'bridge.js', 'host.js', 'host-worker.js']) copyFileSync(join(root, 'web', page), join(out, page));
// The page, marked to be played alone with `--solo`.
const html = readFileSync(join(root, 'web', 'index.html'), 'utf8');
const marked = solo ? html.replace('<html lang="en">', '<html lang="en" data-torque-solo="1">') : html;
if (solo && marked === html) {
  console.error('web/index.html has no <html lang="en"> to mark');
  process.exit(1);
}
writeFileSync(join(out, 'index.html'), marked);
const mib = (file) => (statSync(file).size / 1048576).toFixed(1);
const module = `game_bg.wasm ${mib(moduleFile)} MiB`;
console.log(`${out}: ${module}, ${embed ? 'carrying' : 'fetching'} the pack (${mib(pack)} MiB)${solo ? ', played alone' : ''}`);
