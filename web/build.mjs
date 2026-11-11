// Builds an example for the web into <target>/web/<name>/: the module for wasm32-unknown-unknown, wasm-bindgen's JavaScript
// glue, the page and its file loader, and the example's files, packed by the native build.
//
// Usage: node build.mjs <example> [--embed] [--solo] [--lua] [--dev] [--keep-pack] [--repack] [--client-scripts]
// (level-demo, fps, vehicle-demo, mods-demo; vehicle-demo and mods-demo fetch their files only)
// By default the files are fetched by the page: the folder holds `files.json` (each file's path, size and SHA-256) and
// `game.pack`, which the native build writes (`--web-files`), and the module carries none of them. With `--embed` the module
// carries the pack (the example's `embed-pack` feature; `--web-pack` writes it to <target>/web/<name>.pack) and the folder is
// <target>/web/<name>-embedded/.
// With `--solo` the page is marked to be played alone (its root element's `data-torque-solo`, for a site with no game server:
// the arena then plays on its own server in the page's worker, and no menu offers to host or join) and the folder is
// <target>/web/<name>-solo/ (`-embedded-solo` with `--embed`).
// With `--lua` the page loads the Lua module before the game starts (its root element's `data-torque-lua`, web/lua.js): the
// folder also holds lua.js and torque_lua.wasm, which `just lua-web` builds into <target>/lua-web/. Without it the page never
// fetches either, and a Lua file of the game prints that it cannot run.
// With `--client-scripts` the page runs the Lua a server sends its players (torque-plugin-client-script), if the player allows
// it: the folder also holds client-script.js, client-script-worker.js and the realm guest torque_lua_client.wasm, which `just
// lua-client-web` builds into <target>/lua-web/, and the page (its root element's `data-torque-client-scripts`) starts the
// runner before the game, the guest checked against the list the game's module carries (`acceptedClientGuests`). Without it
// the page fetches none of them and the game tells servers it cannot run their scripts.
//
// With `--dev` the module is built in the `web-dev` profile (the dev profile: incremental, no link-time optimization, the
// third-party crates optimized) and wasm-opt is skipped, for the edit-and-reload loop: the folder is the same, the module is
// larger and slower. A pack the folder already holds (`game.pack` and `files.json`, or the embedded pack file) is kept then, unless a file of
// the example's `game` folder or of `examples/assets` is newer than it, so an edit of Rust alone does not run the native release
// build that packs the files; `--keep-pack` does that without `--dev`, and `--repack` packs again with it. Without these flags the build is the release one: the pack always made again, thin LTO, wasm-opt.
//
// The folder, served as it is: index.html, files.js, loading.js, settings.js, bridge.js, host.js and host-worker.js (a
// browser host), codes.js and invites.js (games joined by invite links), game.js, game_bg.wasm, and files.json with
// game.pack when fetched.
// The target folder is CARGO_TARGET_DIR's, else the workspace's `target`. wasm-bindgen's command line, at the version
// Cargo.lock names, is installed into <target>/web-tools at first.
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { packState } from './pack-stale.mjs';

// Each example's package and program, and whether it can carry its files (`--embed`, its `embed-pack` feature).
const EXAMPLES = {
  'level-demo': { package: 'torque-example-level-demo', bin: 'example-level-demo', embed: true },
  fps: { package: 'torque-example-fps', bin: 'fps-client', embed: true },
  'vehicle-demo': { package: 'torque-example-vehicle-demo', bin: 'vehicle-demo', embed: false },
  // The soccer game's join-only client: the page carries none of the server's mods, it downloads them when it joins.
  'mods-demo': { package: 'torque-example-mods-demo', bin: 'mods-demo-client', embed: false },
};

const name = process.argv[2];
const example = EXAMPLES[name];
const flags = process.argv.slice(3);
const embed = flags.includes('--embed');
const solo = flags.includes('--solo');
const lua = flags.includes('--lua');
const dev = flags.includes('--dev');
const keepPack = flags.includes('--keep-pack') || (dev && !flags.includes('--repack'));
const clientScripts = flags.includes('--client-scripts');
const FLAGS = ['--embed', '--solo', '--lua', '--dev', '--keep-pack', '--repack', '--client-scripts'];
if (!example || flags.some((a) => !FLAGS.includes(a))) {
  console.error(`usage: node build.mjs <${Object.keys(EXAMPLES).join('|')}> [${FLAGS.join('] [')}]`);
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
const luaModule = join(target, 'lua-web', 'torque_lua.wasm');
if (lua && !existsSync(luaModule)) {
  console.error(`${luaModule} is not there: build it with \`just lua-web\` first`);
  process.exit(2);
}
const clientGuest = join(target, 'lua-web', 'torque_lua_client.wasm');
if (clientScripts && !existsSync(clientGuest)) {
  console.error(`${clientGuest} is not there: build it with \`just lua-client-web\` first`);
  process.exit(2);
}
const pack = embed ? join(target, 'web', `${name}.pack`) : join(out, 'game.pack');
const tools = join(target, 'web-tools');
// A program's file name ends as Node's own does (nothing outside Windows).
const suffix = extname(process.execPath);

function run(cmd, args, env = {}) {
  console.log(`> ${cmd} ${args.join(' ')}`);
  const r = spawnSync(cmd, args, { cwd: root, stdio: 'inherit', env: { ...process.env, ...env } });
  if (r.status !== 0) process.exit(r.status ?? 1);
}

// A pack kept from the last build: the folder's files are removed but the pack and its list. It is kept only when it is there
// whole (with its list) and no file of the game's folder or the shared assets is newer than it.
const list = embed ? null : join(out, 'files.json');
const state = keepPack ? packState(pack, list, [join(root, 'examples', name, 'game'), join(root, 'examples', 'assets')]) : null;
const kept = state?.keep === true;
if (keepPack && !kept) console.log(`packing the files: ${state.why}`);
if (kept) {
  console.log(`keeping the pack ${pack}, made ${Math.round(state.age / 60000)} minutes ago (--repack makes it again)`);
  for (const entry of existsSync(out) ? readdirSync(out) : []) {
    if (!embed && (entry === 'game.pack' || entry === 'files.json')) continue;
    rmSync(join(out, entry), { recursive: true, force: true });
  }
} else {
  rmSync(out, { recursive: true, force: true });
  // The pack of an embedded build lives beside the folder: a partial one is never left for the next build to keep.
  if (embed) rmSync(pack, { force: true });
}
mkdirSync(out, { recursive: true });

// The files, packed by the native build: into the folder with their list (written after the pack), or beside it to be embedded.
// A failed or interrupted run leaves no pack and no list behind.
if (!kept) {
  const native = ['run', '-p', example.package, '--release', '--bin', example.bin, '--'];
  const args = [...native, ...(embed ? ['--web-pack', pack] : ['--web-files', out])];
  console.log(`> cargo ${args.join(' ')}`);
  const r = spawnSync('cargo', args, { cwd: root, stdio: 'inherit' });
  if (r.status !== 0) {
    rmSync(pack, { force: true });
    if (list) rmSync(list, { force: true });
    process.exit(r.status ?? 1);
  }
}

// The module, in the `web` profile (release with thin LTO), or `web-dev` with `--dev`. A rustc wrapper (sccache) is turned off,
// set in the config or in RUSTC_WRAPPER: its command lines for web-sys are too long for Windows.
const profile = dev ? 'web-dev' : 'web';
run(
  'cargo',
  ['build', '--config', "build.rustc-wrapper=''", '--profile', profile, '--target', 'wasm32-unknown-unknown',
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

const wasm = join(target, 'wasm32-unknown-unknown', profile, `${example.bin}.wasm`);
run(bindgen, ['--target', 'web', '--no-typescript', '--out-dir', out, '--out-name', 'game', wasm]);

// Binaryen's optimizer over the module when `wasm-opt` is on the PATH (or `WASM_OPT` names it; `WASM_OPT=0` skips it), on the
// features rustc's wasm32 target uses; on two cores. The result must still be a module Node's WebAssembly takes. `--dev` skips it.
const moduleFile = join(out, 'game_bg.wasm');
const opt = dev ? '0' : (process.env.WASM_OPT ?? `wasm-opt${suffix}`);
if (opt !== '0' && spawnSync(opt, ['--version'], { encoding: 'utf8' }).status === 0) {
  const features = ['bulk-memory', 'multivalue', 'mutable-globals', 'nontrapping-float-to-int', 'reference-types', 'sign-ext'];
  run(opt, ['-O2', ...features.map((f) => `--enable-${f}`), moduleFile, '-o', moduleFile], { BINARYEN_CORES: '2' });
  if (!WebAssembly.validate(readFileSync(moduleFile))) {
    console.error(`${moduleFile}: wasm-opt's output is not a valid module`);
    process.exit(1);
  }
} else if (!dev) {
  console.log('wasm-opt not found: the module is not optimized further (install binaryen for a faster one)');
}
const pages = ['files.js', 'loading.js', 'settings.js', 'bridge.js', 'host.js', 'host-worker.js', 'codes.js', 'invites.js'];
for (const page of pages) copyFileSync(join(root, 'web', page), join(out, page));
// The page, marked to be played alone with `--solo`.
const html = readFileSync(join(root, 'web', 'index.html'), 'utf8');
const marked = solo ? html.replace('<html lang="en">', '<html lang="en" data-torque-solo="1">') : html;
// A game with Lua: the page loads its module (and the page's shim) before the game starts.
if (lua) {
  copyFileSync(join(root, 'web', 'lua.js'), join(out, 'lua.js'));
  copyFileSync(luaModule, join(out, 'torque_lua.wasm'));
}

// A game that runs servers' scripts: the page's runner, its Worker and the guest.
if (clientScripts) {
  for (const file of ['client-script.js', 'client-script-worker.js']) copyFileSync(join(root, 'web', file), join(out, file));
  copyFileSync(clientGuest, join(out, 'torque_lua_client.wasm'));
}

if (solo && marked === html) {
  console.error('web/index.html has no <html lang="en"> to mark');
  process.exit(1);
}
const withLua = lua ? marked.replace('<html lang="en"', '<html lang="en" data-torque-lua="1"') : marked;
const page = clientScripts ? withLua.replace('<html lang="en"', '<html lang="en" data-torque-client-scripts="1"') : withLua;
writeFileSync(join(out, 'index.html'), page);
const mib = (file) => (statSync(file).size / 1048576).toFixed(1);
const module = `game_bg.wasm ${mib(moduleFile)} MiB`;
console.log(`${out}: ${module}, ${embed ? 'carrying' : 'fetching'} the pack (${mib(pack)} MiB)${solo ? ', played alone' : ''}${lua ? ', with Lua' : ''}${clientScripts ? ', with server scripts' : ''}${dev ? ', dev profile, not optimized' : ''}`);
