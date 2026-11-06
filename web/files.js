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
// The module checks each file's size, path and SHA-256 against the list. Where the browser has `crypto.subtle` (a secure
// context), the page computes the SHA-256 of the bytes it holds, off the page's thread, and hands the module the digest
// (`addFileDigest`) so the module does not hash them on the main thread (about 750 ms for 23 MiB in wasm). The trust boundary:
// the digest is only ever the browser's own hash of the very bytes handed over in the same call, a file just downloaded or
// one read back from the cache alike, never a value read from the network, the cache or the list. The module still compares it
// to the list's hash, so the list stays the one source of truth. Without `crypto.subtle` (http on a LAN address) or a module
// without `addFileDigest`, the module hashes the bytes itself (`addFile`).
//
// A file taken is kept in the browser's Cache Storage under its hash, so a later load reads it from there and makes no request
// for it; entries the list no longer names are dropped. Cache Storage exists only in a secure context (https, or http on
// localhost); elsewhere every load fetches the files.

// The module refused the list or a file: the message is for the page.
export class Refused extends Error {}

// Fetches and hands over every file of `files.json`; `progress(done, total)` follows the bytes. `game` is the module
// (`manifestMaxBytes`, `readManifest`, `addFile`, and `addFileDigest` where the module has it).
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
      await give(game, f.path, bytes);
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

// Hands the module the file `path`, which it takes or refuses with `Refused`. The SHA-256 is the page's own, computed here from
// `bytes` (see the top of this file).
async function give(game, path, bytes) {
  const digest = game.addFileDigest ? await sha256(bytes) : null;
  refusing(() => (digest ? game.addFileDigest(path, bytes, digest) : game.addFile(path, bytes)));
}

// The SHA-256 of `bytes` by the browser's crypto (null: this context has none, or it failed). `digest` copies the bytes
// before it returns its promise, so they are what is hashed whatever the caller does next.
async function sha256(bytes) {
  const subtle = globalThis.crypto?.subtle;
  if (!subtle) return null;
  try {
    return new Uint8Array(await subtle.digest('SHA-256', bytes));
  } catch (e) {
    console.warn(`the browser could not hash a file, the game will: ${e?.message ?? e}`);
    return null;
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
    await give(game, path, new Uint8Array(await hit.arrayBuffer()));
    return true;
  } catch (e) {
    console.warn(`${path} in the cache was refused, fetching it again: ${e?.message ?? e}`);
    await cache.delete(key);
    return false;
  }
}
