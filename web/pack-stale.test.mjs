// `node --test web/pack-stale.test.mjs`
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, utimesSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { newestUnder, packState } from './pack-stale.mjs';

test('a pack is kept until a game file is newer, and not without its list', () => {
  const dir = mkdtempSync(join(tmpdir(), 'pack-stale-'));
  try {
    const game = join(dir, 'game', 'sub');
    mkdirSync(game, { recursive: true });
    const file = join(game, 'a.txt');
    const pack = join(dir, 'game.pack');
    const list = join(dir, 'files.json');
    writeFileSync(file, 'a');
    writeFileSync(pack, 'p');
    writeFileSync(list, '{}');
    const at = (path, seconds) => utimesSync(path, seconds, seconds);
    at(file, 1000);
    at(pack, 2000);
    const dirs = [join(dir, 'game'), join(dir, 'missing')];
    assert.equal(newestUnder(dirs), 1000 * 1000);
    const kept = packState(pack, list, dirs);
    assert.ok(kept.keep && kept.age > 0);
    at(file, 3000);
    assert.deepEqual(packState(pack, list, dirs), { keep: false, why: 'a game or asset file is newer than the pack' });
    at(file, 1000);
    rmSync(list);
    assert.equal(packState(pack, list, dirs).keep, false);
    assert.equal(packState(pack, null, dirs).keep, true, 'an embedded pack has no list');
    assert.equal(packState(join(dir, 'none.pack'), null, dirs).keep, false);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
