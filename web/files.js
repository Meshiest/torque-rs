// Fetches the files a web build's game reads, by the list beside the page, and hands each to the module, which takes it
// only when its size and SHA-256 are the list's.
//
// A web build's folder: `index.html` (the page), `files.js` (this), `game.js` and `game_bg.wasm` (the module and its glue),
// `files.json` (the list) and the files it names (`game.pack`), all served from one folder. The list is
//   { "files": [ { "path": "game.pack", "size": 2203456, "sha256": "<64 lowercase hex digits>" } ] }
// with each path relative to the list; torque-app's `manifest` module writes and reads it and holds its caps (the text's size,
// the file count, the bytes in all, the paths' and the hashes' form). The module reads the list first, so a list it refuses
// is never fetched from.
//
// A file taken is kept in the browser's Cache Storage under its hash, so a later load reads it from there and makes no request
// for it; entries the list no longer names are dropped. Cache Storage exists only in a secure context (https, or http on
// localhost); elsewhere every load fetches the files.

// The module refused the list or a file: the message is for the page.
export class Refused extends Error {}

// Fetches and hands over every file of `files.json`; `progress(done, total)` follows the bytes. `game` is the module
// (`manifestMaxBytes`, `readManifest`, `addFile`).
export async function fetchFiles(game, progress) {
  // A page's base, or a worker's own address (a browser host's server fetches the same files).
  const list = new URL('files.json', globalThis.document?.baseURI ?? self.location.href);
  const text = new TextDecoder().decode(await download(list, game.manifestMaxBytes()));
  refusing(() => game.readManifest(text));
  // The module has checked the list against its caps.
  const files = JSON.parse(text).files;
  const total = files.reduce((n, f) => n + f.size, 0);
  let done = 0;
  progress(done, total);
  const cache = await openCache(list);
  const keep = new Set();
  for (const f of files) {
    const key = new Request(new URL(`torque-files/${f.sha256}`, list));
    keep.add(key.url);
    if (!(await fromCache(game, cache, key, f.path))) {
      const before = done;
      const bytes = await download(new URL(f.path, list), f.size, (n) => progress((done += n), total));
      refusing(() => game.addFile(f.path, bytes));
      done = before;
      try {
        await cache?.put(key, new Response(bytes));
      } catch (e) {
        console.warn(`${f.path} was not kept in the cache: ${e}`);
      }
    }
    done += f.size;
    progress(done, total);
  }
  if (cache) {
    for (const old of await cache.keys()) if (!keep.has(old.url)) await cache.delete(old);
  }
}

// Runs `call`, a module call that throws its message when it refuses.
function refusing(call) {
  try {
    call();
  } catch (e) {
    throw new Refused(String(e?.message ?? e));
  }
}

// The file `url`'s bytes, refused past `cap` bytes; `step(n)` follows the bytes read.
async function download(url, cap, step) {
  const response = await fetch(url, { cache: 'no-cache' });
  if (!response.ok) throw new Error(`${url.pathname}: HTTP ${response.status}`);
  const reader = response.body.getReader();
  const chunks = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.length;
    if (size > cap) {
      await reader.cancel();
      throw new Refused(`${url.pathname} is over the ${cap} bytes the list allows`);
    }
    chunks.push(value);
    step?.(value.length);
  }
  const bytes = new Uint8Array(size);
  let at = 0;
  for (const c of chunks) {
    bytes.set(c, at);
    at += c.length;
  }
  return bytes;
}

// The cache of the files of the list `list` (null where the page has no Cache Storage).
async function openCache(list) {
  if (!globalThis.caches) return null;
  try {
    return await caches.open(`torque-files ${list.pathname}`);
  } catch {
    return null;
  }
}

// Hands over the file `path` from the cache, under `key` (false: not there, or the module refused it and it is dropped).
async function fromCache(game, cache, key, path) {
  const hit = await cache?.match(key);
  if (!hit) return false;
  try {
    game.addFile(path, new Uint8Array(await hit.arrayBuffer()));
    return true;
  } catch (e) {
    console.warn(`${path} in the cache was refused, fetching it again: ${e?.message ?? e}`);
    await cache.delete(key);
    return false;
  }
}
