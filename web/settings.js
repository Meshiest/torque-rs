// The page's settings: the renderer's quality and the frame-rate display, kept in the browser's storage (`localStorage`) for
// the next visit. No page needed: the tests run it in Node (`node --test web/settings.test.mjs`).
//
// Until the player picks a quality, Firefox starts at Low (its WebGPU spends far more on each pass and draw than Chromium's)
// and every other browser at High. Storage that is missing, full, blocked or holding something else never stops the page: the
// settings are then the defaults, and a change lasts until the page is closed.

export const QUALITIES = ['low', 'medium', 'high', 'ultra'];

// The storage key.
export const KEY = 'torque-settings';

// Whether `userAgent` is a Gecko browser's (Firefox and its kin say `Gecko/<date>`; the others say only "like Gecko").
export function isGecko(userAgent) {
  return /\bGecko\/\d/.test(String(userAgent ?? ''));
}

// The settings before the player picks any (`picked`: whether the player picked the quality).
export function defaults(userAgent) {
  return { quality: isGecko(userAgent) ? 'low' : 'high', picked: false, fps: false };
}

// The settings kept in `storage` (a `localStorage`), over the defaults for `userAgent`; a value that is not a setting's is
// left at its default.
export function load(storage, userAgent) {
  const settings = defaults(userAgent);
  let kept = null;
  try {
    kept = JSON.parse(storage?.getItem(KEY) ?? 'null');
  } catch {
    return settings;
  }
  if (kept && typeof kept === 'object') {
    if (QUALITIES.includes(kept.quality)) Object.assign(settings, { quality: kept.quality, picked: true });
    if (typeof kept.fps === 'boolean') settings.fps = kept.fps;
  }
  return settings;
}

// Keeps `settings` in `storage` (the quality only once the player picked it); false when it could not.
export function save(storage, settings) {
  try {
    storage.setItem(KEY, JSON.stringify({ quality: settings.picked ? settings.quality : undefined, fps: settings.fps }));
    return true;
  } catch {
    return false;
  }
}
