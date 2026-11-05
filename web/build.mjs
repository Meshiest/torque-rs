// Builds an example for the web: its files packed by the native build (`--web-pack`), the module built for
// wasm32-unknown-unknown with the pack embedded, wasm-bindgen's JavaScript glue, and the page, into <target>/web/<name>/.
//
// Usage: node build.mjs <example>      (level-demo)
// The target folder is CARGO_TARGET_DIR's, else the workspace's `target`. wasm-bindgen's command line, at the version
// Cargo.lock names, is installed into <target>/web-tools at first.
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, statSync } from 'node:fs';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const EXAMPLES = {
  'level-demo': { package: 'torque-example-level-demo', bin: 'example-level-demo' },
};

const name = process.argv[2];
const example = EXAMPLES[name];
if (!example) {
  console.error(`usage: node build.mjs <${Object.keys(EXAMPLES).join('|')}>`);
  process.exit(2);
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const target = resolve(process.env.CARGO_TARGET_DIR ?? join(root, 'target'));
const out = join(target, 'web', name);
const pack = join(target, 'web', `${name}.pack`);
const tools = join(target, 'web-tools');
// A program's file name ends as Node's own does (nothing outside Windows).
const suffix = extname(process.execPath);

function run(cmd, args, env = {}) {
  console.log(`> ${cmd} ${args.join(' ')}`);
  const r = spawnSync(cmd, args, { cwd: root, stdio: 'inherit', env: { ...process.env, ...env } });
  if (r.status !== 0) process.exit(r.status ?? 1);
}

// The files, packed by the native build.
run('cargo', ['run', '-p', example.package, '--release', '--bin', example.bin, '--', '--web-pack', pack]);

// The module, the pack embedded. A rustc wrapper (sccache) is turned off: its command lines for web-sys are too long for
// Windows.
run(
  'cargo',
  ['build', '--config', "build.rustc-wrapper=''", '--release', '--target', 'wasm32-unknown-unknown',
    '-p', example.package, '--bin', example.bin],
  { TORQUE_WEB_PACK: pack },
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

rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
const wasm = join(target, 'wasm32-unknown-unknown', 'release', `${example.bin}.wasm`);
run(bindgen, ['--target', 'web', '--no-typescript', '--out-dir', out, '--out-name', 'game', wasm]);
copyFileSync(join(root, 'web', 'index.html'), join(out, 'index.html'));
const size = statSync(join(out, 'game_bg.wasm')).size;
console.log(`${out}: game_bg.wasm ${(size / 1048576).toFixed(1)} MiB (the pack ${(statSync(pack).size / 1048576).toFixed(1)} MiB)`);
