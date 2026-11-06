// Serves a folder over HTTP on 127.0.0.1 (a web build of an example: its page, script and module; or the site `site.mjs`
// builds, under the prefix a project site has: `/torque-rs/`).
// Usage: node serve.mjs <folder> [port] [prefix]
// A folder's path without its closing `/` is sent there (as a static host does), so a page's relative links resolve.
import { createServer } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { extname, join, normalize, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const types = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.wasm': 'application/wasm',
  '.json': 'application/json',
  '.png': 'image/png',
  '.webp': 'image/webp',
  '.svg': 'image/svg+xml',
  '.ico': 'image/x-icon',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.ttf': 'font/ttf',
  '.txt': 'text/plain; charset=utf-8',
};

// Serves `folder` on 127.0.0.1:`port` (0: any free port) under `prefix` (`/`, or a path like `/torque-rs/`: anything outside
// it is not found); resolves to the listening server.
export function serve(folder, port, prefix = '/') {
  const root = resolve(folder);
  const base = `/${prefix.replace(/^\/+|\/+$/g, '')}/`.replace('//', '/');
  const server = createServer(async (req, res) => {
    let path;
    try {
      path = decodeURIComponent(new URL(req.url, 'http://localhost').pathname);
    } catch {
      res.writeHead(400).end('bad request');
      return;
    }
    if (`${path}/` === base) {
      res.writeHead(301, { Location: base + new URL(req.url, 'http://localhost').search }).end();
      return;
    }
    if (!path.startsWith(base)) {
      res.writeHead(404).end('not found');
      return;
    }
    path = path.slice(base.length - 1);
    const file = normalize(join(root, path.endsWith('/') ? path + 'index.html' : path));
    if (file !== root && !file.startsWith(root + sep)) {
      res.writeHead(403).end();
      return;
    }
    try {
      if (!path.endsWith('/') && (await stat(file)).isDirectory()) {
        const query = new URL(req.url, 'http://localhost').search;
        // Leading slashes collapsed: `//name` must not become a link to another host.
        res.writeHead(301, { Location: `${base}${path.replace(/^\/+/, '')}/${query}` }).end();
        return;
      }
      const body = await readFile(file);
      res.writeHead(200, {
        'Content-Type': types[extname(file)] ?? 'application/octet-stream',
        // the page shows a download's percent by it
        'Content-Length': body.length,
        'Cache-Control': 'no-cache',
      });
      res.end(body);
    } catch {
      res.writeHead(404).end('not found');
    }
  });
  return new Promise((ok) => server.listen(port, '127.0.0.1', () => ok(server)));
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const folder = process.argv[2] ?? '.';
  const prefix = process.argv[4] ?? '/';
  const server = await serve(folder, Number(process.argv[3] ?? 8080), prefix);
  const base = `/${prefix.replace(/^\/+|\/+$/g, '')}/`.replace('//', '/');
  console.log(`http://127.0.0.1:${server.address().port}${base} serves ${resolve(folder)}`);
}
