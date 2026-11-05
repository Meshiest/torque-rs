//! The race track of the hills: a closed loop of packed brown dirt, about 2 km round and [`WIDTH`] m wide, east of the start
//! field (it passes the field's edge on a straight), over the round hill's north shoulder, out round the west, south and east of
//! the map and back.
//!
//! The loop is a centripetal Catmull-Rom curve through [`CONTROL`], sampled every [`STEP`] m. [`Track::nearest`] finds the
//! closest point of the centerline (a grid of buckets keeps that cheap for the terrain's 65,536 samples). The terrain paints it
//! with its second material, `dirt`: [`dirt_share`] is that material's weight at a point, a ramp in the distance from the
//! centerline that crosses one half at the track's edge (the renderer sharpens a terrain layer's weight around one half, so a
//! ramp over the coarse samples still makes an edge about a meter soft). [`dirt_uv`] gives the dirt's texture coordinates
//! along the track and across it, so the worn bands of `dirt.png` follow the curves.
//!
//! **The ground under it** ([`Track::ground`]): the hills' height along the centerline averaged over a few tens of meters,
//! plus the course's own landforms ([`LANDFORMS`]: crests the buggy goes light over, dips, a shallow valley it runs along the
//! floor of and a ridge it runs along the top of), each spread [`LIFT_FULL`] to [`LIFT_END`] m either side so the hills
//! around rise and fall with it. Across the track the ground is a straight line leaning with the hillside it runs along
//! ([`LEAN_MAX`]) and banked in the turns ([`BANK_GAIN`]), at most [`BANK_MAX`] in all, eased back into the hills beside it.
//! The start straight and the way over the round hill ([`SHAPED`]) keep the hills' own shape, their turns unbanked.

use std::sync::OnceLock;

/// The track's width (m), and half of it.
pub const WIDTH: f32 = 13.0;
pub const HALF_WIDTH: f32 = WIDTH / 2.0;
/// The meters of distance over which the dirt's weight falls from one to none (it is one half at the edge).
pub const RAMP: f32 = 10.0;
/// The centerline's sample spacing (m).
pub const STEP: f32 = 2.0;
/// The farthest from the centerline [`Track::nearest`] answers (m): past it there is no track.
pub const REACH: f32 = 48.0;
/// How far beside the track its smoothing reaches: full to [`HALF_WIDTH`] + `SMOOTH_FULL`, none from [`HALF_WIDTH`] +
/// `SMOOTH_END`; and the spread (standard deviation, m) of the average along it.
pub const SMOOTH_FULL: f32 = 2.0;
pub const SMOOTH_END: f32 = 14.0;
pub const SMOOTH_ALONG: f32 = 10.0;
/// How far beside the track a landform reaches: full to `LIFT_FULL` m from the centerline, none from `LIFT_END` m (inside
/// [`REACH`]).
pub const LIFT_FULL: f32 = 14.0;
pub const LIFT_END: f32 = 46.0;
/// The bank: the slope across the track per unit of curvature (1/m; a turn of radius r leans `BANK_GAIN / r`), at most
/// `BANK_MAX`, the curvature averaged over `BANK_ALONG` m (a standard deviation).
pub const BANK_GAIN: f32 = 7.0;
pub const BANK_MAX: f32 = 0.1;
pub const BANK_ALONG: f32 = 12.0;
/// The hills' slope across the track that it follows, at most (measured `LEAN_SPAN` m either side of the centerline).
pub const LEAN_MAX: f32 = 0.08;
pub const LEAN_SPAN: f32 = 8.0;
/// Where the course has its own shape, in control points ([`CONTROL`]'s indices, fractions between them): from `SHAPED.0`
/// to `SHAPED.1`, eased in and out over [`SHAPED_EASE`] m. Outside it (the start straight and the round hill's shoulder)
/// the ground keeps the hills' shape, its turns unbanked.
pub const SHAPED: (f32, f32) = (5.6, 23.2);
pub const SHAPED_EASE: f32 = 40.0;

/// One of the course's landforms: where along it (in control points, as [`SHAPED`]) and how high (m; below zero a dip).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Landform {
    /// A smooth hump (a half cosine wave) `half` m either side of its top: a crest (`height` above zero) or a dip.
    Hump { at: f32, half: f32, height: f32 },
    /// A long rise or fall: `height` from `from` to `to`, eased in before and out after over `ease` m (a ridge above zero,
    /// a valley below).
    Run {
        from: f32,
        to: f32,
        ease: f32,
        height: f32,
    },
}

/// The course's landforms, in driving order: a crest after the hill, a dip past the north-west turn, a shallow valley down
/// the west side, a ridge along the south with two crests on top, and a dip and a crest on the way back.
pub const LANDFORMS: [Landform; 8] = [
    Landform::Hump {
        at: 6.4,
        half: 32.0,
        height: 3.0,
    },
    Landform::Hump {
        at: 8.75,
        half: 32.0,
        height: -2.6,
    },
    Landform::Run {
        from: 9.4,
        to: 11.6,
        ease: 80.0,
        height: -4.0,
    },
    Landform::Run {
        from: 13.2,
        to: 15.6,
        ease: 90.0,
        height: 4.5,
    },
    Landform::Hump {
        at: 14.0,
        half: 30.0,
        height: 2.6,
    },
    Landform::Hump {
        at: 15.15,
        half: 26.0,
        height: 2.0,
    },
    Landform::Hump {
        at: 19.55,
        half: 30.0,
        height: -2.0,
    },
    Landform::Hump {
        at: 20.55,
        half: 32.0,
        height: 2.6,
    },
];

/// The dirt texture's repeat along the track (about this, made a whole number of repeats round the loop) and its span across
/// it (one repeat, centered on the centerline).
pub const DIRT_REPEAT: f32 = 8.0;
pub const DIRT_ACROSS: f32 = 16.0;

/// The curve's control points (m), in driving order: up the straight east of the field, over the hill's north shoulder, round
/// the west, the south and the east, and into the straight again.
pub const CONTROL: [[f32; 2]; 25] = [
    [62.0, -40.0],
    [64.0, 10.0],
    [58.0, 58.0],
    [36.0, 98.0],
    [4.0, 122.0],
    [-50.0, 140.0],
    [-140.0, 200.0],
    [-250.0, 200.0],
    [-330.0, 120.0],
    [-345.0, 10.0],
    [-300.0, -80.0],
    [-225.0, -125.0],
    [-205.0, -215.0],
    [-120.0, -290.0],
    [0.0, -275.0],
    [105.0, -330.0],
    [225.0, -300.0],
    [310.0, -215.0],
    [328.0, -125.0],
    [285.0, -50.0],
    [210.0, -55.0],
    [152.0, -100.0],
    [114.0, -122.0],
    [84.0, -117.0],
    [66.0, -94.0],
];

/// The bucket size of the centerline's grid (m).
const CELL: f32 = 32.0;

/// The closest point of the centerline to a point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Near {
    /// The distance to the centerline (m).
    pub distance: f32,
    /// How far along the loop the closest point is (m, from the first control point, in `[0, length)`).
    pub along: f32,
    /// The signed distance across: positive on the right of the driving direction.
    pub side: f32,
}

/// The sampled loop (module docs).
#[derive(Debug, Clone)]
pub struct Track {
    points: Vec<[f32; 2]>,
    /// The distance along the loop of each point.
    along: Vec<f32>,
    length: f32,
    /// The raw ground's height along the centerline averaged ([`SMOOTH_ALONG`]), at each point.
    profile: Vec<f32>,
    /// The distance along the loop of each control point.
    knots: Vec<f32>,
    /// The bank of the turn at each point and the hills' own slope across the track there (each the slope across, rising to the
    /// right of the driving direction).
    turn: Vec<f32>,
    lean: Vec<f32>,
    /// The buckets: the segments (by their first point) within [`REACH`] of each cell.
    grid: Vec<Vec<u32>>,
    grid_min: [f32; 2],
    grid_side: [usize; 2],
}

fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn len(a: [f32; 2]) -> f32 {
    a[0].hypot(a[1])
}

/// The centripetal Catmull-Rom curve through the closed `control`, sampled about every `step`, and the index of each control
/// point's sample.
fn sample(control: &[[f32; 2]], step: f32) -> (Vec<[f32; 2]>, Vec<usize>) {
    let n = control.len();
    let mut out = Vec::new();
    let mut starts = Vec::with_capacity(n);
    for i in 0..n {
        starts.push(out.len());
        let p = [n - 1, 0, 1, 2].map(|k| control[(i + k) % n]);
        let knot = |t: f32, a: [f32; 2], b: [f32; 2]| t + len(sub(b, a)).sqrt();
        let t0 = 0.0;
        let t1 = knot(t0, p[0], p[1]);
        let t2 = knot(t1, p[1], p[2]);
        let t3 = knot(t2, p[2], p[3]);
        let lerp = |a: [f32; 2], b: [f32; 2], ta: f32, tb: f32, t: f32| {
            let (wa, wb) = ((tb - t) / (tb - ta), (t - ta) / (tb - ta));
            [a[0] * wa + b[0] * wb, a[1] * wa + b[1] * wb]
        };
        let count = ((len(sub(p[2], p[1])) / step) as usize).max(2);
        for k in 0..count {
            let t = t1 + (t2 - t1) * k as f32 / count as f32;
            let a1 = lerp(p[0], p[1], t0, t1, t);
            let a2 = lerp(p[1], p[2], t1, t2, t);
            let a3 = lerp(p[2], p[3], t2, t3, t);
            let b1 = lerp(a1, a2, t0, t2, t);
            let b2 = lerp(a2, a3, t1, t3, t);
            out.push(lerp(b1, b2, t1, t2, t));
        }
    }
    (out, starts)
}

/// A Gaussian average of the closed sequence `v` (samples [`STEP`] apart) over `spread` m, three spreads each way.
fn smoothed(v: &[f32], spread: f32) -> Vec<f32> {
    let n = v.len();
    let reach = (3.0 * spread / STEP).ceil() as isize;
    (0..n)
        .map(|i| {
            let (mut sum, mut weight) = (0.0, 0.0);
            for k in -reach..=reach {
                let j = (i as isize + k).rem_euclid(n as isize) as usize;
                let d = k as f32 * STEP / spread;
                let w = (-0.5 * d * d).exp();
                sum += v[j] * w;
                weight += w;
            }
            sum / weight
        })
        .collect()
}

impl Track {
    fn new() -> Track {
        let (points, starts) = sample(&CONTROL, STEP);
        let n = points.len();
        let mut along = Vec::with_capacity(n);
        let mut s = 0.0;
        for i in 0..n {
            along.push(s);
            s += len(sub(points[(i + 1) % n], points[i]));
        }
        let length = s;
        // The buckets over the points' box grown by the reach.
        let mut lo = [f32::INFINITY; 2];
        let mut hi = [f32::NEG_INFINITY; 2];
        for p in &points {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k] - REACH);
                hi[k] = hi[k].max(p[k] + REACH);
            }
        }
        let side = [0, 1].map(|k| ((hi[k] - lo[k]) / CELL).ceil() as usize + 1);
        let mut grid = vec![Vec::new(); side[0] * side[1]];
        for i in 0..n {
            let (a, b) = (points[i], points[(i + 1) % n]);
            let cell = |v: f32, k: usize| ((v - lo[k]) / CELL).floor().max(0.0) as usize;
            let (x0, x1) = (
                cell(a[0].min(b[0]) - REACH, 0),
                cell(a[0].max(b[0]) + REACH, 0),
            );
            let (y0, y1) = (
                cell(a[1].min(b[1]) - REACH, 1),
                cell(a[1].max(b[1]) + REACH, 1),
            );
            for y in y0..=y1.min(side[1] - 1) {
                for x in x0..=x1.min(side[0] - 1) {
                    grid[x + y * side[0]].push(i as u32);
                }
            }
        }
        // The raw ground along the centerline, averaged.
        let raw: Vec<f32> = points
            .iter()
            .map(|p| crate::terrain::raw_height(p[0], p[1]))
            .collect();
        let profile = smoothed(&raw, SMOOTH_ALONG);
        // The curvature at each point (positive turning left), averaged, as a bank raising the outside of the turn.
        let curvature: Vec<f32> = (0..n)
            .map(|i| {
                let (a, b, c) = (points[(i + n - 1) % n], points[i], points[(i + 1) % n]);
                let (u, v) = (sub(b, a), sub(c, b));
                let turn = (u[0] * v[1] - u[1] * v[0]).atan2(u[0] * v[0] + u[1] * v[1]);
                turn / (0.5 * (len(u) + len(v))).max(1e-3)
            })
            .collect();
        let turn = smoothed(&curvature, BANK_ALONG)
            .into_iter()
            .map(|k| (BANK_GAIN * k).clamp(-BANK_MAX, BANK_MAX))
            .collect();
        // The hills' own slope across the track (rising to the right), averaged: the track leans with the hillside it runs
        // along rather than cutting a flat shelf into it (a shelf's edge launched whatever drove across it).
        let across: Vec<f32> = (0..n)
            .map(|i| {
                let h = sub(points[(i + 1) % n], points[(i + n - 1) % n]);
                let l = len(h).max(1e-6);
                let right = [h[1] / l, -h[0] / l];
                let at = |d: f32| {
                    let p = points[i];
                    crate::terrain::raw_height(p[0] + right[0] * d, p[1] + right[1] * d)
                };
                (at(LEAN_SPAN) - at(-LEAN_SPAN)) / (2.0 * LEAN_SPAN)
            })
            .collect();
        let lean = smoothed(&across, BANK_ALONG)
            .into_iter()
            .map(|k| k.clamp(-LEAN_MAX, LEAN_MAX))
            .collect();
        let knots = starts.iter().map(|&i| along[i]).collect();
        Track {
            points,
            along,
            length,
            profile,
            knots,
            turn,
            lean,
            grid,
            grid_min: lo,
            grid_side: side,
        }
    }

    /// The loop's length (m).
    pub fn length(&self) -> f32 {
        self.length
    }

    /// The centerline's points, in driving order.
    pub fn points(&self) -> &[[f32; 2]] {
        &self.points
    }

    /// The closest point of the centerline to (x, y), None past [`REACH`].
    pub fn nearest(&self, x: f32, y: f32) -> Option<Near> {
        let cx = ((x - self.grid_min[0]) / CELL).floor();
        let cy = ((y - self.grid_min[1]) / CELL).floor();
        if !(cx >= 0.0 && cy >= 0.0) {
            return None;
        }
        let (cx, cy) = (cx as usize, cy as usize);
        if cx >= self.grid_side[0] || cy >= self.grid_side[1] {
            return None;
        }
        let n = self.points.len();
        let mut best: Option<(f32, usize, f32)> = None;
        for &i in &self.grid[cx + cy * self.grid_side[0]] {
            let i = i as usize;
            let (a, b) = (self.points[i], self.points[(i + 1) % n]);
            let ab = sub(b, a);
            let ap = sub([x, y], a);
            let l2 = (ab[0] * ab[0] + ab[1] * ab[1]).max(1e-12);
            let t = ((ap[0] * ab[0] + ap[1] * ab[1]) / l2).clamp(0.0, 1.0);
            let d = len(sub(ap, [ab[0] * t, ab[1] * t]));
            if best.is_none_or(|(bd, _, _)| d < bd) {
                best = Some((d, i, t));
            }
        }
        let (distance, i, t) = best.filter(|(d, _, _)| *d <= REACH)?;
        let (a, b) = (self.points[i], self.points[(i + 1) % n]);
        let ab = sub(b, a);
        let seg = len(ab).max(1e-6);
        let cross = (ab[0] * (y - a[1]) - ab[1] * (x - a[0])) / seg;
        Some(Near {
            distance,
            along: (self.along[i] + t * seg) % self.length,
            // The left of the direction is the positive cross product.
            side: -cross,
        })
    }

    /// The centerline's point `along` m round the loop and its unit heading.
    pub fn at(&self, along: f32) -> ([f32; 2], [f32; 2]) {
        let s = along.rem_euclid(self.length);
        let i = self.along.partition_point(|&a| a <= s).saturating_sub(1);
        let n = self.points.len();
        let (a, b) = (self.points[i], self.points[(i + 1) % n]);
        let ab = sub(b, a);
        let seg = len(ab).max(1e-6);
        let t = ((s - self.along[i]) / seg).clamp(0.0, 1.0);
        (
            [a[0] + ab[0] * t, a[1] + ab[1] * t],
            [ab[0] / seg, ab[1] / seg],
        )
    }

    /// `v` (a value at each point) `along` m round the loop, between its points.
    fn lerp_at(&self, v: &[f32], along: f32) -> f32 {
        let s = along.rem_euclid(self.length);
        let i = self.along.partition_point(|&a| a <= s).saturating_sub(1);
        let n = self.points.len();
        let next = if i + 1 == n {
            self.length
        } else {
            self.along[i + 1]
        };
        let t = ((s - self.along[i]) / (next - self.along[i]).max(1e-6)).clamp(0.0, 1.0);
        v[i] * (1.0 - t) + v[(i + 1) % n] * t
    }

    /// Where `knot` (a control point's index, fractions between them) lies along the loop (m).
    pub fn knot(&self, knot: f32) -> f32 {
        let n = self.knots.len();
        let k = knot.rem_euclid(n as f32);
        let i = (k.floor() as usize).min(n - 1);
        let next = if i + 1 == n {
            self.length
        } else {
            self.knots[i + 1]
        };
        self.knots[i] + (next - self.knots[i]) * (k - i as f32)
    }

    /// The signed distance from `from` to `to` round the loop, the shorter way.
    fn between(&self, from: f32, to: f32) -> f32 {
        let d = (to - from).rem_euclid(self.length);
        if d > self.length * 0.5 {
            d - self.length
        } else {
            d
        }
    }

    /// How much of the course's own shape there is `along` m round the loop: 1 inside [`SHAPED`], easing to none at its ends.
    pub fn shaped(&self, along: f32) -> f32 {
        let from = self.knot(SHAPED.0);
        let span = (self.knot(SHAPED.1) - from).rem_euclid(self.length);
        let into = (along - from).rem_euclid(self.length);
        if into > span {
            return 0.0;
        }
        let ease = |d: f32| crate::terrain::smoothstep(0.0, SHAPED_EASE, d);
        ease(into) * ease(span - into)
    }

    /// The landforms' height `along` m round the loop (m, added to the hills).
    pub fn lift(&self, along: f32) -> f32 {
        let shaped = self.shaped(along);
        if shaped == 0.0 {
            return 0.0;
        }
        let sum: f32 = LANDFORMS
            .iter()
            .map(|f| match *f {
                Landform::Hump { at, half, height } => {
                    let u = self.between(self.knot(at), along) / half;
                    if u.abs() < 1.0 {
                        height * 0.5 * (1.0 + (std::f32::consts::PI * u).cos())
                    } else {
                        0.0
                    }
                }
                Landform::Run {
                    from,
                    to,
                    ease,
                    height,
                } => {
                    let into = self.between(self.knot(from), along);
                    let left = self.between(along, self.knot(to));
                    height
                        * crate::terrain::smoothstep(-ease, 0.0, into)
                        * crate::terrain::smoothstep(-ease, 0.0, left)
                }
            })
            .sum();
        sum * shaped
    }

    /// The bank `along` m round the loop: the slope across the track, rising to the right of the driving direction (the
    /// hillside's lean and the turn's bank, at most [`BANK_MAX`]).
    pub fn bank_at(&self, along: f32) -> f32 {
        (self.lerp_at(&self.lean, along) + self.lerp_at(&self.turn, along) * self.shaped(along))
            .clamp(-BANK_MAX, BANK_MAX)
    }

    /// The track's height on its centerline `along` m round the loop: the hills averaged along it, and the landforms.
    pub fn surface(&self, along: f32) -> f32 {
        self.lerp_at(&self.profile, along) + self.lift(along)
    }

    /// The ground at (x, y) with the course's shape over `raw` (the hills' own height there): the landforms spread over the
    /// hills beside the track ([`LIFT_FULL`] to [`LIFT_END`]); on the track its surface, banked, eased back into those hills
    /// beside it ([`SMOOTH_FULL`] to [`SMOOTH_END`] past its edge).
    pub fn ground(&self, x: f32, y: f32, raw: f32) -> f32 {
        let Some(near) = self.nearest(x, y) else {
            return raw;
        };
        let lift = self.lift(near.along);
        let around =
            raw + lift * (1.0 - crate::terrain::smoothstep(LIFT_FULL, LIFT_END, near.distance));
        let edge = HALF_WIDTH + SMOOTH_FULL;
        let surface =
            self.surface(near.along) + self.bank_at(near.along) * near.side.clamp(-edge, edge);
        let keep = crate::terrain::smoothstep(edge, HALF_WIDTH + SMOOTH_END, near.distance);
        surface + (around - surface) * keep
    }
}

/// The track, made once.
pub fn track() -> &'static Track {
    static TRACK: OnceLock<Track> = OnceLock::new();
    TRACK.get_or_init(Track::new)
}

/// The dirt's weight at (x, y), 0 to 1: one half at the track's edge, rising toward the centerline and falling away from it
/// over [`RAMP`] m a unit.
pub fn dirt_share(x: f32, y: f32) -> f32 {
    track().nearest(x, y).map_or(0.0, |n| {
        (0.5 + (HALF_WIDTH - n.distance) / RAMP).clamp(0.0, 1.0)
    })
}

/// The dirt texture's coordinates at the corners of a triangle of the ground: along the track (a whole number of repeats round
/// the loop) and across it ([`DIRT_ACROSS`] a repeat, the centerline in the middle); a corner's distance along is taken
/// within half a loop of the first corner's, so a triangle across the loop's start does not run back round it. A corner past
/// [`REACH`] takes the world's coordinates.
pub fn dirt_uv(corners: &[[f32; 3]; 3]) -> [[f32; 2]; 3] {
    let t = track();
    let repeat = t.length / (t.length / DIRT_REPEAT).round().max(1.0);
    let near = corners.map(|c| t.nearest(c[0], c[1]));
    let first = near.iter().flatten().next().map(|n| n.along);
    let mut out = [[0.0; 2]; 3];
    for (k, c) in corners.iter().enumerate() {
        out[k] = match (near[k], first) {
            (Some(n), Some(f)) => {
                let mut s = n.along;
                if s - f > t.length * 0.5 {
                    s -= t.length;
                } else if f - s > t.length * 0.5 {
                    s += t.length;
                }
                [s / repeat, n.side / DIRT_ACROSS + 0.5]
            }
            _ => [c[0] / repeat, -c[1] / repeat],
        };
    }
    out
}
