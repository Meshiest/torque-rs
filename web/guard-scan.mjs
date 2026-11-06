// A lint against honest mistakes in our own scripts: it fails when a script in the repository could open a browser without
// `guard.mjs`'s `launchGuarded`. It is not a defense against someone who means to get around it: escaped or computed module
// names, `eval` and the like are not chased. The real protection is the page-level override `launchGuarded` installs in every
// page it opens (web/guard.mjs, tested by web/guard.test.mjs); this only keeps a script from opening a browser the other way by
// accident.
//
// Every `.mjs`, `.js`, `.cjs`, `.ts`, `.mts`, `.cts`, `.jsx` and `.tsx` file outside node_modules, target and scratch folders is
// read with its comments removed. Except the two files `web/guard.mjs` and `web/guard-scan.mjs` (by their exact paths), none
// may name a browser driver (puppeteer, playwright, selenium, ...), a browser program or its debugging switches,
// `createCDPSession`, `createRequire`, or a `require` or `import` of a name that is not a plain string; `child_process` is
// allowed only in the scripts listed in `SPAWNERS` (they start cargo, the servers and the checks, never a browser).
// `web/guard.mjs` itself must still install the pointer-lock block.
// Usage: node guard-scan.mjs [root]      (run by `just examples/web-guard` and CI)
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(process.argv[2] ?? join(dirname(fileURLToPath(import.meta.url)), '..'));
const SKIP = new Set(['node_modules', 'target', '.git', '.scratch', '.claude']);
const SOURCE = /\.(mjs|js|cjs|ts|mts|cts|jsx|tsx)$/;
// The scripts that may use `child_process` (by their path under the root).
const SPAWNERS = new Set(['web/build.mjs', 'web/site.mjs', 'web/fps-check.mjs', 'web/p2p-check.mjs']);
const GUARD = 'web/guard.mjs';
const SCAN = 'web/guard-scan.mjs';

function* walk(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (!SKIP.has(entry.name)) yield* walk(join(dir, entry.name));
    } else if (SOURCE.test(entry.name)) {
      yield join(dir, entry.name);
    }
  }
}

// What may stand just before a regular expression literal: after these a `/` starts one; after a name, a number or a closing
// bracket it divides.
const BEFORE_REGEX = /[(,=:[!&|?{};+*%<>~^-]$|(?:^|[^\w$.])(?:return|typeof|case|in|of|delete|void|throw|new|else|do|yield|await)$/;

// The code without comments: line and block comments are cut outside strings and regular expressions, so a name inside a
// comment is not a use, and a comment marker inside a string or a regular expression (`/\//`) is not a comment.
export function stripComments(text) {
  let out = '';
  let i = 0;
  while (i < text.length) {
    const c = text[i];
    const next = text[i + 1];
    if (c === '/' && next === '/') {
      while (i < text.length && text[i] !== '\n') i++;
    } else if (c === '/' && next === '*') {
      const end = text.indexOf('*/', i + 2);
      i = end < 0 ? text.length : end + 2;
      out += ' ';
    } else if (c === '"' || c === "'" || c === '`') {
      let j = i + 1;
      while (j < text.length && text[j] !== c) j += text[j] === '\\' ? 2 : 1;
      out += text.slice(i, j + 1);
      i = j + 1;
    } else if (c === '/' && (out.trimEnd() === '' || BEFORE_REGEX.test(out.trimEnd()))) {
      let j = i + 1;
      let inClass = false;
      while (j < text.length && text[j] !== '\n' && (inClass || text[j] !== '/')) {
        if (text[j] === '\\') j++;
        else if (text[j] === '[') inClass = true;
        else if (text[j] === ']') inClass = false;
        j++;
      }
      out += text.slice(i, j + 1);
      i = j + 1;
    } else {
      out += c;
      i++;
    }
  }
  return out;
}

const BROWSER =
  /puppeteer|playwright|selenium|webdriver|chromedriver|chrome|chromium|msedge|firefox|brave|remote-debugging|devtools-protocol|createCDPSession|\bcdp\b/i;
const COMPUTED_LOAD = /\bcreateRequire\b|\bimport\s*\(\s*[^'"`\s]|\brequire\s*\(\s*[^'"`\s]/;
const SPAWN = /\bchild_process\b/;

// Run as a program (not when another script imports `stripComments`).
if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  const problems = [];
  let scanned = 0;
  for (const file of walk(root)) {
    const name = relative(root, file).replaceAll('\\', '/');
    const code = stripComments(readFileSync(file, 'utf8'));
    scanned++;
    if (name === GUARD) {
      for (const need of ['evaluateOnNewDocument', 'requestPointerLock', 'configurable: false']) {
        if (!code.includes(need)) problems.push(`${name}: no longer has \`${need}\` (the pointer-lock block)`);
      }
      continue;
    }
    if (name === SCAN) continue;
    const named = BROWSER.exec(code);
    if (named) problems.push(`${name}: names a browser ("${named[0]}"); a browser comes only from launchGuarded in ${GUARD}`);
    if (COMPUTED_LOAD.test(code)) problems.push(`${name}: loads a module by a computed name (createRequire, import(expr), require(expr))`);
    if (SPAWN.test(code) && !SPAWNERS.has(name)) problems.push(`${name}: uses child_process (only ${[...SPAWNERS].join(', ')} may)`);
  }
  if (problems.length) {
    console.error(`browser scripts must get their browser from launchGuarded (${GUARD}):\n  ${problems.join('\n  ')}`);
    process.exit(1);
  }
  console.log(`guard-scan: ${scanned} scripts, none opens a browser except through launchGuarded`);
}
