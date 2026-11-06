// Builds the project's website into one folder, ready for a static host (GitHub Pages): the gallery (web/gallery.html) as
// its index.html with the examples' pictures beside it, each web example played alone in its own folder (`build.mjs <name>
// --solo`: files fetched by URL), the guide (mdBook) in book/ and the API reference (rustdoc) in api/. Every link is
// relative, so the folder works under a project site's path (`/torque-rs/`) and from a local server
// (`node web/serve.mjs <folder> 8080 /torque-rs/`).
//
// Usage: node site.mjs [folder]      (default: <target>/site; the target folder is CARGO_TARGET_DIR's, else `target`)
// Needs what build.mjs needs (rustup's wasm32-unknown-unknown target, Node), and mdbook on the PATH.
import { spawnSync } from 'node:child_process';
import { copyFileSync, cpSync, existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// The examples on the gallery, by build.mjs's names; each folder in the site is named the same.
const EXAMPLES = ['level-demo', 'fps', 'vehicle-demo'];
// The pictures the gallery shows (examples/screenshots).
const PICTURES = [
  'level-demo-plaza.webp',
  'level-demo-office.webp',
  'level-demo-valley.webp',
  'fps-shot.webp',
  'vehicle-demo-turn.webp',
];
// The crate the API reference opens on.
const FRONT_CRATE = 'torque';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const target = resolve(process.env.CARGO_TARGET_DIR ?? join(root, 'target'));
const site = resolve(process.argv[2] ?? join(target, 'site'));
if (process.argv.length > 3) {
  console.error('usage: node site.mjs [folder]');
  process.exit(2);
}

function run(cmd, args) {
  console.log(`> ${cmd} ${args.join(' ')}`);
  const r = spawnSync(cmd, args, { cwd: root, stdio: 'inherit' });
  if (r.status !== 0) process.exit(r.status ?? 1);
}

rmSync(site, { recursive: true, force: true });
mkdirSync(site, { recursive: true });

for (const name of EXAMPLES) {
  run(process.execPath, [join(root, 'web', 'build.mjs'), name, '--solo']);
  cpSync(join(target, 'web', `${name}-solo`), join(site, name), { recursive: true });
}

run('mdbook', ['build', join(root, 'book'), '--dest-dir', join(site, 'book')]);

run('cargo', ['doc', '--workspace', '--no-deps', '--all-features']);
cpSync(join(target, 'doc'), join(site, 'api'), { recursive: true });
if (!existsSync(join(site, 'api', FRONT_CRATE, 'index.html'))) {
  console.error(`the API reference has no ${FRONT_CRATE}/index.html`);
  process.exit(1);
}
// rustdoc writes no front page for a workspace: this one opens the umbrella crate's.
writeFileSync(
  join(site, 'api', 'index.html'),
  `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="refresh" content="0; url=${FRONT_CRATE}/index.html">
<title>torque-rs API docs</title>
<link rel="icon" href="data:,">
</head>
<body><p><a href="${FRONT_CRATE}/index.html">The API docs of the ${FRONT_CRATE} crate</a></p></body>
</html>
`,
);

copyFileSync(join(root, 'web', 'gallery.html'), join(site, 'index.html'));
mkdirSync(join(site, 'screenshots'));
for (const picture of PICTURES) {
  copyFileSync(join(root, 'examples', 'screenshots', picture), join(site, 'screenshots', picture));
}
console.log(`${site}: the gallery, ${EXAMPLES.join(', ')}, book/ and api/`);
