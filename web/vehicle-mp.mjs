// The vehicle demo's two-player run for `p2p-check.mjs`: two pages in one game (`first` joined before `second`, so its
// own buggy is the map's and `second` stands on that buggy's right). The first gets in at the wheel and the second takes
// the passenger's seat of the same buggy; the first drives, and both must see it moved and agree on where it is; the
// second gets out, the first drives on, and the second, on foot, must see the buggy where the first does; then the
// second walks. Getting in is the scripts' own `look` and `getIn` (what the mouse's move and
// click call), through the page's `torque.call`: no mouse is moved or clicked. Each step adds to `problems` and stops at
// the first failure.
import { writeFile } from 'node:fs/promises';

const pause = (ms) => new Promise((r) => setTimeout(r, ms));
const near = (a, b, d) => a.length === 3 && b.length === 3 && Math.hypot(a[0] - b[0], a[1] - b[1]) < d;
const apart = (a, b) => (a.length === 3 && b.length === 3 ? Math.hypot(a[0] - b[0], a[1] - b[1]) : 0);
const text = (p) => p.map((v) => v.toFixed(1)).join(' ');

// `page` holds `key` for `ms`.
async function hold(page, key, ms) {
  await (await page.$('canvas'))?.focus();
  await page.keyboard.down(key);
  await pause(ms);
  await page.keyboard.up(key);
}

// `page`'s figure looks down at the buggy beside it and asks for a seat; true once it has `seat`.
async function getIn(page, seat, wait) {
  await page.evaluate(() => globalThis.torque.call('look', '110'));
  await pause(500);
  await page.evaluate(() => globalThis.torque.call('getIn', '1'));
  const s = await wait(page, 8000, (s) => s.seat === seat);
  return s.seat === seat;
}

// At the wheel of `page`: W for 2.5 s, then the brake for 1.5 s; where its client then sees the buggy (after a moment).
async function drive(page, state) {
  await hold(page, 'KeyW', 2500);
  await hold(page, 'Space', 1500);
  await pause(1000);
  return (await state(page)).buggy;
}

// `state(page)` reads a page's marks now, `wait(page, ms, done)` until `done(state)` (or `ms`, or a problem).
export async function vehicleRun({ first, second, state, wait, problems, out }) {
  const shot = async (page, name) => {
    const canvas = await page.$('canvas');
    if (canvas) await writeFile(`${out.replace(/\.png$/, '')}-${name}.png`, await canvas.screenshot({ type: 'png' }));
  };
  const fail = (line) => {
    problems.push(line);
    return false;
  };
  // 1. Both get in: the first at the wheel, the second in the passenger's seat of the same buggy.
  if (!(await getIn(first, 0, wait))) return fail('the first player did not get in at the wheel');
  if (!(await getIn(second, 1, wait))) return fail("the second player did not take the passenger's seat");
  await pause(1500);
  let b = await state(second);
  let a = await state(first);
  if (!near(a.buggy, b.buggy, 1.5)) return fail(`the two are not in one buggy (${text(a.buggy)}, ${text(b.buggy)})`);
  console.log(`both seated: the first at the wheel, the second the passenger, at ${text(a.buggy)}`);
  // 2. The first drives both; each sees the buggy go, and they agree on where it is.
  const from = a.buggy;
  const to = await drive(first, state);
  b = await wait(second, 5000, (s) => near(s.buggy, to, 2.5));
  console.log(`with a passenger: the first sees it at ${text(to)}, the passenger at ${text(b.buggy)}`);
  if (!(apart(from, to) > 5)) return fail(`the buggy did not move with a passenger (${text(from)} to ${text(to)})`);
  if (!(apart(from, b.buggy) > 5) || !near(b.buggy, to, 2.5)) {
    return fail(`the passenger does not see the buggy go (${text(b.buggy)}, the driver sees ${text(to)})`);
  }
  await shot(second, 'passenger');
  await shot(first, 'driver');
  // 3. The second gets out; the first drives on, and the second, on foot, sees the buggy go.
  await hold(second, 'KeyE', 200);
  b = await wait(second, 8000, (s) => s.seat === -1 && s.figure);
  if (b.seat !== -1) return fail('the passenger did not get out');
  await pause(1000);
  const off = await drive(first, state);
  console.log(`the first drove on from ${text(to)} to ${text(off)}`);
  if (!(apart(to, off) > 5)) return fail(`the buggy did not move (${text(to)} to ${text(off)})`);
  b = await wait(second, 5000, (s) => s.buggies.some((p) => near(p, off, 2.5)));
  if (!b.buggies.some((p) => near(p, off, 2.5))) {
    return fail(`the second does not see the buggy where the first does (${text(off)}; it sees ${JSON.stringify(b.buggies)})`);
  }
  console.log(`the second, on foot (seat ${b.seat}), sees it there`);
  await shot(second, 'seen-driving');
  // 4. The second walks.
  const at = (await state(second)).position;
  await hold(second, 'KeyW', 2000);
  await pause(1000);
  b = await state(second);
  const walked = apart(at, b.position);
  console.log(`on foot, W held 2 s: the second walked ${walked.toFixed(2)}`);
  if (!(walked > 1)) return fail(`the second player did not walk (moved ${walked})`);
  await shot(second, 'walked');
  return true;
}
