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

// The frame-time display's line, from the game's `data-torque-fps` and `data-torque-frame-ms` (`avg max`).
export function fpsText(fps, frameMs) {
  const [avg, max] = String(frameMs ?? '').split(' ').map(Number);
  if (!fps || !Number.isFinite(avg)) return 'fps: waiting for frames';
  const worst = Number.isFinite(max) ? `, worst ${max.toFixed(1)} ms` : '';
  return `${Number(fps).toFixed(0)} fps, ${avg.toFixed(2)} ms a frame${worst}`;
}
