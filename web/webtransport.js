// A page joining the server that served it opens a WebTransport session to it first, when it can: the page is a secure
// context (`https`, or `localhost`), the browser has the API, the server put `webtransport.json` beside the page (the QUIC
// port number, the CONNECT's path and the SHA-256 hashes of its short certificates; a server without the QUIC link serves none),
// and the session opens within a few seconds. Then the game's datagrams ride the session (the game's `webTransportOpened`,
// `webTransportDatagram`, `webTransportEnded`: torque-app's `web_join`) and no game socket is opened. Otherwise the page says
// why on the root element's `data-torque-wt` and the game joins over its game socket and a data channel as before.
//
// `serverCertificateHashes` makes the browser take the server's self-signed certificate by its hash alone (the server
// rotates them, so the page always fetches the file fresh), and `requireUnreliable: true` fails the session rather than fall
// back to HTTP/2, which carries no datagrams. `?nowt` on the page's address skips the session (the fallback's check).

/// How long the session has to open before the page joins over its game socket instead.
export const OPEN_TIMEOUT_MS = 5000;

const bytesOf = (hex) => {
  if (typeof hex !== 'string' || hex.length !== 64 || /[^0-9a-f]/i.test(hex)) throw new Error('a hash is not 32 bytes of hex');
  return Uint8Array.from(hex.match(/../g), (h) => parseInt(h, 16));
};

/**
 * Reads the server's `webtransport.json` beside the page: `{ port, path, hashes }`, or null when the server serves none (or
 * it is not a sensible file).
 */
export async function readInfo(baseURI = document.baseURI) {
  const response = await fetch(new URL('webtransport.json', baseURI), { cache: 'no-store' });
  if (!response.ok) return null;
  const info = await response.json();
  if (!Number.isInteger(info?.port) || info.port < 1 || info.port > 65535) return null;
  if (typeof info.path !== 'string' || !info.path.startsWith('/')) return null;
  if (!Array.isArray(info.hashes) || info.hashes.length === 0 || info.hashes.length > 8) return null;
  return info;
}

/** The session's address: the page's host at the server's QUIC port number, over `https`. */
export function sessionUrl(info, location = globalThis.location) {
  const host = location.hostname.includes(':') ? `[${location.hostname}]` : location.hostname;
  return `https://${host}:${info.port}${info.path}`;
}

/**
 * Opens the session and hands it to `game` (the module's exports). Resolves to `'open'`, or to why the page stays on its game
 * socket. Never throws.
 */
export async function joinOverWebTransport(game, { timeoutMs = OPEN_TIMEOUT_MS, mark = (why) => (globalThis.document.documentElement.dataset.torqueWt = why) } = {}) {
  const why = await open(game, timeoutMs);
  mark(why);
  return why;
}

async function open(game, timeoutMs) {
  if (typeof game.webTransportOpened !== 'function') return 'no-game-support';
  if (new URLSearchParams(globalThis.location?.search ?? '').has('nowt')) return 'off';
  if (!('WebTransport' in globalThis)) return 'no-api';
  if (!globalThis.isSecureContext) return 'insecure-page';
  let info;
  try {
    info = await readInfo();
  } catch {
    return 'no-info';
  }
  if (!info) return 'no-info';
  let wt;
  try {
    wt = new WebTransport(sessionUrl(info), {
      serverCertificateHashes: info.hashes.map((h) => ({ algorithm: 'sha-256', value: bytesOf(h) })),
      requireUnreliable: true,
    });
  } catch (e) {
    console.warn(`WebTransport is not available here: ${e?.message ?? e}`);
    return 'refused';
  }
  const timeout = new Promise((_, fail) => setTimeout(() => fail(new Error('timed out')), timeoutMs));
  try {
    await Promise.race([wt.ready, timeout]);
  } catch (e) {
    console.warn(`the WebTransport session did not open: ${e?.message ?? e}`);
    try {
      wt.close();
    } catch {}
    return 'failed';
  }
  let ended = false;
  const end = (why) => {
    if (ended) return;
    ended = true;
    game.webTransportEnded(why);
  };
  const writer = wt.datagrams.writable.getWriter();
  const send = (bytes) => {
    if (ended) return;
    // The browser's own queue is the back-pressure: a datagram it has no room for is dropped, as on UDP.
    if (writer.desiredSize !== null && writer.desiredSize <= 0) return;
    writer.write(bytes).catch(() => {});
  };
  (async () => {
    const reader = wt.datagrams.readable.getReader();
    try {
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        game.webTransportDatagram(value);
      }
    } catch {}
    end('The server closed the connection.');
  })();
  wt.closed.then(
    () => end('The server closed the connection.'),
    (e) => end(`The connection to the server was lost: ${e?.message ?? e}`),
  );
  game.webTransportOpened(send, wt.datagrams.maxDatagramSize || 1200);
  return 'open';
}
