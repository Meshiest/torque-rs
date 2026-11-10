// Builds the web examples and prints the size of each module (`game_bg.wasm`), or its change against an earlier run.
//
// Usage: node size.mjs [--out <file>] [--compare <baseline.json>] [--limit <percent>]
// Builds level-demo, fps and vehicle-demo with build.mjs (their files fetched, none embedded), so the numbers are the modules
// a page downloads. The sizes are written as JSON ({"level-demo": bytes, ...}) to <file> when `--out` names one, and always
// to <target>/web/size.json. With `--compare` each line also gives the change against the baseline in percent; with `--limit`
// the exit code is 1 when any module grew or shrank by more than that many percent, or is missing from the baseline.
import { spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const EXAMPLES = ['level-demo', 'fps', 'vehicle-demo'];

const args = process.argv.slice(2);
const value = (flag) => {
  const at = args.indexOf(flag);
  if (at < 0) return undefined;
  const v = args[at + 1];
  if (v === undefined || v.startsWith('--')) {
    console.error(`${flag} needs a value`);
    process.exit(2);
  }
  return v;
};
const known = new Set(['--out', '--compare', '--limit']);
const stray = args.filter((a, i) => !known.has(a) && !known.has(args[i - 1]));
if (stray.length > 0 || args.some((a) => a.startsWith('--') && !known.has(a))) {
  console.error('usage: node size.mjs [--out <file>] [--compare <baseline.json>] [--limit <percent>]');
  process.exit(2);
}
const outFile = value('--out');
const compareFile = value('--compare');
const limit = value('--limit') === undefined ? undefined : Number(value('--limit'));
if (limit !== undefined && !(limit >= 0)) {
  console.error('--limit needs a number of percent');
  process.exit(2);
}
const baseline = compareFile === undefined ? undefined : JSON.parse(readFileSync(resolve(compareFile), 'utf8'));

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '..');
const target = resolve(process.env.CARGO_TARGET_DIR ?? join(root, 'target'));

const sizes = {};
for (const name of EXAMPLES) {
  const built = spawnSync(process.execPath, [join(here, 'build.mjs'), name], {
    cwd: root,
    encoding: 'utf8',
    maxBuffer: 1 << 28,
  });
  if (built.status !== 0) {
    process.stderr.write(`${built.stdout}${built.stderr}`);
    console.error(`building ${name} failed`);
    process.exit(1);
  }
  sizes[name] = statSync(join(target, 'web', name, 'game_bg.wasm')).size;
}

let failed = false;
for (const [name, bytes] of Object.entries(sizes)) {
  let line = `${name.padEnd(13)} ${String(bytes).padStart(10)} bytes  ${(bytes / 1048576).toFixed(2)} MiB`;
  if (baseline !== undefined) {
    const before = baseline[name];
    if (typeof before === 'number' && before > 0) {
      const change = ((bytes - before) / before) * 100;
      line += `  ${change >= 0 ? '+' : ''}${change.toFixed(2)}% (was ${before})`;
      if (limit !== undefined && Math.abs(change) > limit) failed = true;
    } else {
      line += '  (not in the baseline)';
      if (limit !== undefined) failed = true;
    }
  }
  console.log(line);
}

const json = `${JSON.stringify(sizes, null, 2)}\n`;
const record = join(target, 'web', 'size.json');
mkdirSync(dirname(record), { recursive: true });
writeFileSync(record, json);
if (outFile !== undefined) {
  mkdirSync(dirname(resolve(outFile)), { recursive: true });
  writeFileSync(resolve(outFile), json);
}
if (failed) {
  console.error(`a module changed by more than ${limit}% or is missing from the baseline`);
  process.exit(1);
}
