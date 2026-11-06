// The page's fetching of the game's files and the digest it hands the module, without a browser: `node --test web/files.test.mjs`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { Refused, fetchFiles } from './files.js';

const BASE = 'http://127.0.0.1:9/build/';
const hexOf = (bytes) => createHash('sha256').update(bytes).digest('hex');
const PACK = new Uint8Array([1, 2, 3, 4, 5]);
const LIST = JSON.stringify({ files: [{ path: 'game.pack', size: PACK.length, sha256: hexOf(PACK) }] });

// A module that checks what it is given as the real one does: the size and the list's hash. `withDigest` false: an older
// module that has only `addFile`.
function stubModule({ withDigest = true } = {}) {
  const calls = [];
  const check = (path, bytes, hash) => {
    if (path !== 'game.pack') throw new Error(`${path} is not in the list of files`);
    if (bytes.length !== PACK.length) throw new Error('wrong size');
    if (hash !== hexOf(PACK)) throw new Error(`${path} is not the file the list names`);
  };
  const module = {
    calls,
    manifestMaxBytes: () => 1 << 16,
    readManifest: () => {},
    addFile(path, bytes) {
      calls.push('addFile');
      check(path, bytes, hexOf(bytes));
    },
  };
  if (withDigest) {
    module.addFileDigest = (path, bytes, digest) => {
      calls.push(['addFileDigest', digest.length]);
      if (digest.length !== 32) throw new Error('a SHA-256 digest has 32 bytes');
      check(path, bytes, Buffer.from(digest).toString('hex'));
    };
  }
  return module;
}

// A Cache Storage in memory, by URL.
function stubCaches() {
  const stores = new Map();
  return {
    stores,
    async open(name) {
      if (!stores.has(name)) stores.set(name, new Map());
      const entries = stores.get(name);
      return {
        match: async (req) => (entries.has(req.url) ? new Response(entries.get(req.url)) : undefined),
        put: async (req, res) => void entries.set(req.url, new Uint8Array(await res.arrayBuffer())),
        keys: async () => [...entries.keys()].map((url) => new Request(url)),
        delete: async (req) => entries.delete(req.url),
      };
    },
  };
}

// Runs `body` with fetch, the page's base and (optionally) a cache and crypto in place; `requests` lists the URLs fetched.
async function withPage({ caches, crypto, packBytes = PACK }, body) {
  const saved = Object.fromEntries(['fetch', 'document', 'caches', 'crypto'].map((k) => [k, Object.getOwnPropertyDescriptor(globalThis, k)]));
  const requests = [];
  const define = (k, value) => Object.defineProperty(globalThis, k, { value, configurable: true, writable: true });
  define('document', { baseURI: BASE });
  define('caches', caches);
  if (crypto !== undefined) define('crypto', crypto);
  define('fetch', async (url) => {
    requests.push(String(url));
    const body = String(url).endsWith('files.json') ? LIST : packBytes;
    return new Response(body);
  });
  try {
    await body(requests);
  } finally {
    for (const [k, d] of Object.entries(saved)) d ? Object.defineProperty(globalThis, k, d) : delete globalThis[k];
  }
}

test('a downloaded file is handed over with the browser digest, not hashed by the module', async () => {
  const game = stubModule();
  await withPage({}, async () => {
    await fetchFiles(game, () => {});
  });
  assert.deepEqual(game.calls, [['addFileDigest', 32]]);
});

test('a file from the cache goes through the same digest check', async () => {
  const caches = stubCaches();
  await withPage({ caches }, () => fetchFiles(stubModule(), () => {}));
  const game = stubModule();
  await withPage({ caches }, async (requests) => {
    await fetchFiles(game, () => {});
    assert.deepEqual(requests.filter((u) => u.endsWith('game.pack')), [], 'read from the cache');
  });
  assert.deepEqual(game.calls, [['addFileDigest', 32]]);
});

test('a changed file in the cache is refused by the digest of its bytes, dropped and fetched again', async () => {
  const caches = stubCaches();
  await withPage({ caches }, () => fetchFiles(stubModule(), () => {}));
  const [entries] = [...caches.stores.values()];
  const [key] = [...entries.keys()];
  entries.set(key, new Uint8Array([9, 9, 9, 9, 9]));
  const game = stubModule();
  const warn = console.warn;
  console.warn = () => {};
  try {
    await withPage({ caches }, async (requests) => {
      await fetchFiles(game, () => {});
      assert.equal(requests.filter((u) => u.endsWith('game.pack')).length, 1, 'fetched again');
    });
  } finally {
    console.warn = warn;
  }
  assert.deepEqual(game.calls, [['addFileDigest', 32], ['addFileDigest', 32]]);
  assert.deepEqual([...entries.get(key)], [...PACK], 'the good bytes are cached again');
});

test('a downloaded file the module refuses is a Refused', async () => {
  await withPage({ packBytes: new Uint8Array([1, 2, 3, 4, 6]) }, async () => {
    await assert.rejects(fetchFiles(stubModule(), () => {}), (e) => e instanceof Refused && /not the file the list names/.test(e.message));
  });
});

test('without crypto.subtle the module hashes the file itself', async () => {
  const game = stubModule();
  await withPage({ crypto: {} }, () => fetchFiles(game, () => {}));
  assert.deepEqual(game.calls, ['addFile']);
});

test('a digest that fails falls back to the module hashing the file', async () => {
  const game = stubModule();
  const warn = console.warn;
  console.warn = () => {};
  try {
    await withPage({ crypto: { subtle: { digest: async () => { throw new Error('no'); } } } }, () => fetchFiles(game, () => {}));
  } finally {
    console.warn = warn;
  }
  assert.deepEqual(game.calls, ['addFile']);
});

test('a module without addFileDigest is handed the file with addFile', async () => {
  const game = stubModule({ withDigest: false });
  await withPage({}, () => fetchFiles(game, () => {}));
  assert.deepEqual(game.calls, ['addFile']);
});
