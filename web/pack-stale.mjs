// Whether a pack made by an earlier build is still the one the game's files would make: used by `build.mjs --keep-pack`.
import { existsSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';

// The newest modification time (ms) of any file under `dirs` (folders that are not there count for nothing; `target` and
// `.scratch` folders are not looked into), or 0 when there is none.
export function newestUnder(dirs) {
  let newest = 0;
  const walk = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.isDirectory()) {
        if (entry.name !== 'target' && entry.name !== '.scratch') walk(join(dir, entry.name));
      } else {
        newest = Math.max(newest, statSync(join(dir, entry.name)).mtimeMs);
      }
    }
  };
  for (const dir of dirs) if (existsSync(dir)) walk(dir);
  return newest;
}

// What to do with the pack `pack` (its list `list`, none for an embedded pack) beside the game's folders `dirs`:
// `{ keep: true, age }` (age in ms), or `{ keep: false, why }`.
export function packState(pack, list, dirs) {
  if (!existsSync(pack)) return { keep: false, why: 'there is no pack yet' };
  if (list && !existsSync(list)) return { keep: false, why: 'the pack has no list' };
  const made = statSync(pack).mtimeMs;
  if (newestUnder(dirs) > made) return { keep: false, why: 'a game or asset file is newer than the pack' };
  return { keep: true, age: Date.now() - made };
}
