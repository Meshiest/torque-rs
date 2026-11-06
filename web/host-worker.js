// The browser host's server, in a Web Worker (a background tab slows its page's timers, not a worker's). It fetches the
// game's files as the page does (from the cache after the page's first load), starts the server on the module's relay
// transport, and runs its ticks; the page (`host.js`) holds every link and passes their messages both ways.
//
// From the page: first { start: { wasm } } (the module compiled, or null: the worker compiles it), then { open: id, ip },
// { from: id, data: ArrayBuffer }, { close: id }. To the page: { started: map } or { failed: message }, { to: id, data:
// ArrayBuffer }, { closed: id } (a link the server closed for breaking a cap). The map is the worker's `?map=` (empty: the
// first).
import init, * as game from './game.js';
import { fetchFiles } from './files.js';

const early = [];
let ready = false;
let begun = false;

function handle(m) {
  if ('open' in m) {
    if (!game.hostOpen(m.open, String(m.ip))) postMessage({ closed: m.open });
  } else if ('from' in m) {
    if (!game.hostDatagram(m.from, new Uint8Array(m.data))) postMessage({ closed: m.from });
  } else if ('close' in m) {
    game.hostClose(m.close);
  }
}

onmessage = (e) => {
  if (e.data && 'start' in e.data) return begin(e.data.start?.wasm);
  if (ready) handle(e.data);
  else if (early.length < 4096) early.push(e.data);
};

function tick() {
  const out = game.hostTick();
  for (let i = 0; i + 1 < out.length; i += 2) {
    const data = out[i + 1];
    postMessage({ to: out[i], data: data.buffer }, [data.buffer]);
  }
  for (const id of game.hostClosed()) postMessage({ closed: id });
}

async function begin(wasm) {
  if (begun) return;
  begun = true;
  try {
    await init(wasm instanceof WebAssembly.Module ? { module_or_path: wasm } : undefined);
    await fetchFiles(game, () => {});
    const map = game.hostStart(new URL(self.location.href).searchParams.get('map') ?? '');
    ready = true;
    for (const m of early.splice(0)) handle(m);
    setInterval(tick, 8);
    postMessage({ started: map });
  } catch (e) {
    postMessage({ failed: String(e?.message ?? e) });
  }
}
