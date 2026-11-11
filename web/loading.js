// The page's loading display: its stages in order and the line each shows, and the module's download read in pieces so its
// bytes can be shown as they come. No page needed: the tests run it in Node (`node --test web/loading.test.mjs`).
//
// The stages: the page downloads the module (`module`), compiles it (`compile`), fetches the game's files (`files`: the pack,
// skipped by a build that carries it) and starts the game (`start`: its server and the map's mission, run before the first
// frame); the game then asks for the GPU (`gpu`), loads the map until it holds the player (`map`), and draws (`ready`, the
// display gone). The page sets the first four; the game sets the rest on the root element's `data-torque-stage`.

export const STAGES = ['module', 'compile', 'files', 'start', 'gpu', 'map', 'ready'];

const LABELS = {
  module: 'Downloading the game',
  compile: 'Compiling the game',
  files: "Loading the game's files",
  start: 'Starting the game',
  gpu: 'Starting the GPU',
  map: 'Loading the map',
  ready: 'Ready',
};

const mib = (n) => (n / 1048576).toFixed(1);

// The line a stage shows: its name, its step (`3 of 7`) and, for a download, the bytes so far of `total` (unknown: null) and
// the percent.
export function stageText(stage, done = null, total = null) {
  const at = STAGES.indexOf(stage);
  if (at < 0) return stage;
  let line = `${LABELS[stage]} (${at + 1} of ${STAGES.length})`;
  if (done !== null) {
    line += total ? `: ${mib(done)} of ${mib(total)} MiB, ${percent(done, total)}%` : `: ${mib(done)} MiB`;
  }
  return line;
}

// `done` of `total` as a whole percent, 0 to 100.
export function percent(done, total) {
  if (!(total > 0)) return 0;
  return Math.max(0, Math.min(100, Math.floor((done * 100) / total)));
}

// Shows `done` of `total` bytes on the loading bar `bar` (its fill, the first child, scaled; its `aria-valuenow` for a screen
// reader), or with no total a stage whose length is not known: the bar's `busy` look, a piece that slides across. Both are drawn
// by a transform alone, which the browser animates off the page's thread: the bar keeps moving while the game's work holds it.
export function showBar(bar, done = null, total = null) {
  const fill = bar.firstElementChild;
  if (done !== null && total) {
    const p = percent(done, total);
    bar.classList.remove('busy');
    bar.setAttribute('aria-valuenow', String(p));
    if (fill) fill.style.transform = `scaleX(${p / 100})`;
  } else {
    bar.classList.add('busy');
    bar.removeAttribute('aria-valuenow');
    if (fill) fill.style.transform = '';
  }
}

// Whether the game's stage `next` (from `data-torque-stage`) is a later one than `now`: a stage never goes back.
export function later(now, next) {
  return STAGES.indexOf(next) > STAGES.indexOf(now);
}

// The bytes of `response`, read in pieces; `progress(done, total)` follows them (`total` from Content-Length, null without
// one; a compressed body's may be its compressed size, so `done` is held at most `total`).
export async function readStreamed(response, progress) {
  if (!response.ok) throw new Error(`${new URL(response.url || 'http://x/').pathname}: HTTP ${response.status}`);
  const length = Number(response.headers.get('Content-Length'));
  const total = Number.isFinite(length) && length > 0 ? length : null;
  progress?.(0, total);
  if (!response.body) {
    const bytes = new Uint8Array(await response.arrayBuffer());
    progress?.(bytes.length, total ?? bytes.length);
    return bytes;
  }
  const reader = response.body.getReader();
  const chunks = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    size += value.length;
    progress?.(total ? Math.min(size, total) : size, total);
  }
  const bytes = new Uint8Array(size);
  let at = 0;
  for (const c of chunks) {
    bytes.set(c, at);
    at += c.length;
  }
  return bytes;
}

// The compiled module of the WebAssembly file at `url`. Where the browser compiles a stream (`WebAssembly.compileStreaming`) the
// body is split in two: one branch feeds `progress(done, total)`, the other the compiler, so compiling runs while the rest is
// still downloading and the bytes are never gathered here. `compiling()` is called when the download is over and only the
// compile is left. A browser without it, or a stream that fails, downloads the file again and compiles the bytes. `fetcher` is
// `fetch`, for a test.
export async function loadModule(url, progress, compiling, fetcher = fetch) {
  let response = await fetcher(url);
  if (response.ok && response.body && typeof WebAssembly.compileStreaming === 'function') {
    try {
      const [shown, fed] = response.body.tee();
      const length = Number(response.headers.get('Content-Length'));
      const total = Number.isFinite(length) && length > 0 ? length : null;
      const compile = WebAssembly.compileStreaming(new Response(fed, { headers: { 'Content-Type': 'application/wasm' } }));
      // Whichever branch fails first is the error; the other's outcome is not left unhandled.
      const results = Promise.allSettled([follow(shown, total, progress), compile]);
      const [followed, compiled] = await results;
      if (followed.status === 'rejected') throw followed.reason;
      compiling();
      if (compiled.status === 'rejected') throw compiled.reason;
      return compiled.value;
    } catch (error) {
      console.warn(`the module was not compiled while it downloaded, downloading it again: ${error?.message ?? error}`);
      response = await fetcher(url);
    }
  }
  const bytes = await readStreamed(response, progress);
  compiling();
  await new Promise(requestAnimationFrame);
  return WebAssembly.compile(bytes);
}

// Counts the bytes of `stream` as they come (`total`: as in `readStreamed`).
async function follow(stream, total, progress) {
  progress?.(0, total);
  const reader = stream.getReader();
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.length;
    progress?.(total ? Math.min(size, total) : size, total);
  }
}

// The frame-time display's line, from the game's `data-torque-fps` and `data-torque-frame-ms` (`avg max`).
export function fpsText(fps, frameMs) {
  const [avg, max] = String(frameMs ?? '').split(' ').map(Number);
  if (!fps || !Number.isFinite(avg)) return 'fps: waiting for frames';
  const worst = Number.isFinite(max) ? `, worst ${max.toFixed(1)} ms` : '';
  return `${Number(fps).toFixed(0)} fps, ${avg.toFixed(2)} ms a frame${worst}`;
}
