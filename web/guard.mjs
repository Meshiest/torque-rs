// The browser checks never let a page take the pointer lock: headless Chrome on Windows confines the real cursor to its
// invisible window when a page locks the pointer, and a check that clicks the canvas would do exactly that. Every page a
// check loads carries `?nolock` (the page's `index.html` then replaces `requestPointerLock` with a no-op), and a check opens
// pages only through `open`, which refuses a URL without it. `guard-scan.mjs` fails any check script that does otherwise.

/** `url` with the `nolock` option on its query. */
export function noLock(url) {
  const u = new URL(url);
  u.searchParams.set('nolock', '1');
  return u.href;
}

/** Throws unless `url` carries the `nolock` option. */
export function assertNoLock(url) {
  if (!new URL(url).searchParams.has('nolock')) {
    throw new Error(`refusing to load ${url} without ?nolock (a page's pointer lock confines the real cursor)`);
  }
}

/** `page.goto`, for a URL with `nolock` only. */
export async function open(page, url, options) {
  assertNoLock(url);
  return page.goto(url, options);
}
