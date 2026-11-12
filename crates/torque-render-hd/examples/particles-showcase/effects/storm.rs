//! A lightning storm at night: a low deck of dark cloud over a field in the rain, lit only when a bolt strikes. Each strike
//! is built fresh: a channel from the cloud base to the ground made by splitting its line and pushing the halves aside,
//! with branches that leave it on the way down (and branches off those). The leader comes down in a frame, then the return
//! stroke lights the whole channel white and flashes the whole scene, sky and all, hard ([`Effect::flash`]), flickers twice
//! as it fades, and is gone within a third of a second; the branches die with the leader, the main channel's halo lingers
//! as an afterglow, and sparks fly where it hit, bouncing on the ground ([`Grit`]). Far off, sheet lightning flickers inside
//! the cloud now and then. Space calls a strike.

use glam::Vec3;
use torque_render_scene::hd::{DynamicObject, HdLight};
use torque_render_scene::particles::{EmitterData, ParticleData, ParticleExtras, Rng};
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Draw, Vertex};

use super::{
    Effect, Floor, Grit, GritKind, Lcg, Node, Setting, emitter, hash, light, one, particle, plain,
    within_cone,
};
use crate::scenery::{self, beam, canopy, cuboid, ribbon, vertex};
use crate::sprites::{DROP, GLOW, LINE};

const EYE: Vec3 = Vec3::new(-3.0, -16.0, 2.0);
/// The cloud base's height and the deck's reach each way.
const CLOUD_BASE: f32 = 19.0;
const DECK: (f32, f32, f32, f32) = (-70.0, 70.0, -40.0, 90.0);
/// A dead tree's trunk: where it stands and how tall it is (the bolt's favorite).
const TREE: Vec3 = Vec3::new(3.5, 14.0, 0.0);
const TREE_HEIGHT: f32 = 7.5;
/// When the return stroke follows the leader's start (a frame), how long a strike shows, and when its restrikes come (seconds
/// after the stroke) and how bright they are against the stroke's 1.
const STROKE_AT: f32 = 0.035;
const STRIKE_S: f32 = 0.6;
const RESTRIKES: [(f32, f32); 2] = [(0.1, 0.75), (0.21, 0.5)];
/// The stroke's brightness on the channel (the sprite's 1 is white; past it the core burns wider).
const STROKE: f32 = 2.6;
/// The sparks' throw where the bolt hits: speed, its spread, the cone (degrees).
const SPARKS: (f32, f32, f32) = (6.0, 3.0, 70.0);
/// The rain: its box over the field (x and y each way, the height), drops a second and their speed.
const RAIN: (f32, f32, f32, f32, f32) = (-22.0, 22.0, -20.0, 40.0, 12.0);
const RAIN_RATE: f32 = 1500.0;
const RAIN_SPEED: f32 = 9.0;

/// One piece of a bolt: its ends, its width against the main channel's, how far down the main channel it hangs (0 at the
/// cloud, 1 at the ground: the leader reaches it in that order) and whether it is a branch.
struct Segment {
    a: Vec3,
    b: Vec3,
    width: f32,
    down: f32,
    branch: bool,
}

struct Bolt {
    t0: f32,
    hit: Vec3,
    segments: Vec<Segment>,
}

/// A bolt being built: its generator and the pieces so far.
struct Grow {
    g: Lcg,
    segments: Vec<Segment>,
}

impl Grow {
    /// The channel from `a` to `b` split `levels` times, each split's middle pushed aside; `down` runs over it. A branch may
    /// leave each piece of the main channel and, more rarely, of a branch.
    fn channel(
        &mut self,
        a: Vec3,
        b: Vec3,
        width: f32,
        down: (f32, f32),
        levels: u32,
        branch: bool,
    ) {
        let (d0, d1) = down;
        if levels == 0 {
            self.segments.push(Segment {
                a,
                b,
                width,
                down: d0,
                branch,
            });
            let dir = (b - a).normalize_or(-Vec3::Z);
            let chance = if branch { 0.03 } else { 0.08 };
            if width > 0.2 && self.g.unit() < chance {
                // A branch: off the channel's way by 20 to 50 degrees, a third to a half as long as what is left below.
                let side = dir
                    .cross(Vec3::new(
                        self.g.range(-1.0, 1.0),
                        self.g.range(-1.0, 1.0),
                        0.2,
                    ))
                    .normalize_or(Vec3::X);
                let turn = self.g.range(0.35, 0.9);
                let way = (dir * turn.cos() + side * turn.sin()).normalize_or(dir);
                let reach = (b.z * self.g.range(0.18, 0.35)).max(1.0);
                let end = b + way * reach;
                let end = Vec3::new(end.x, end.y, end.z.max(0.3));
                self.channel(b, end, width * 0.35, (d0, d1 + 0.15), 4, true);
            }
            return;
        }
        let mid = (a + b) * 0.5;
        let along = b - a;
        let len = along.length();
        let dir = along.normalize_or(-Vec3::Z);
        let side = dir
            .cross(Vec3::new(
                self.g.range(-1.0, 1.0),
                self.g.range(-1.0, 1.0),
                self.g.range(-0.3, 0.3),
            ))
            .normalize_or(Vec3::X);
        let mid = mid + side * (len * self.g.range(0.08, 0.22));
        let dm = (d0 + d1) * 0.5;
        self.channel(a, mid, width, (d0, dm), levels - 1, branch);
        self.channel(mid, b, width, (dm, d1), levels - 1, branch);
    }
}

impl Bolt {
    /// A bolt from the cloud base over `hit` (a little aside) down to it.
    fn new(t0: f32, hit: Vec3, seed: u64) -> Bolt {
        let mut grow = Grow {
            g: Lcg(seed),
            segments: Vec::new(),
        };
        let top = Vec3::new(
            hit.x + grow.g.range(-5.0, 5.0),
            hit.y + grow.g.range(-4.0, 4.0),
            CLOUD_BASE + 0.5,
        );
        grow.channel(top, hit, 1.0, (0.0, 1.0), 7, false);
        Bolt {
            t0,
            hit,
            segments: grow.segments,
        }
    }

    /// How bright the main channel and the branches are at age `a` (0..1 and above: the stroke's white), and how much of
    /// the leader has come down (0..1 of `down`).
    fn glow(a: f32) -> (f32, f32, f32) {
        let reach = (a / STROKE_AT).clamp(0.0, 1.0);
        if a < STROKE_AT {
            return (0.3, 0.25, reach);
        }
        let since = a - STROKE_AT;
        let mut main = STROKE * (-since / 0.045).exp();
        for (at, peak) in RESTRIKES {
            if since >= at {
                main += STROKE * peak * (-(since - at) / 0.035).exp();
            }
        }
        // The afterglow: the channel's air stays faintly lit as it cools.
        main += 0.18 * (1.0 - since / (STRIKE_S - STROKE_AT)).clamp(0.0, 1.0);
        let branches = 0.9 * (-since / 0.035).exp();
        (main, branches, 1.0)
    }

    fn shape(&self, t: f32, out: &mut Vec<Vertex>) {
        let a = t - self.t0;
        let (main, branches, reach) = Bolt::glow(a);
        let scale = |rgb: [f32; 3], k: f32| {
            [
                (rgb[0] * k * 255.0).min(255.0) as u8,
                (rgb[1] * k * 255.0).min(255.0) as u8,
                (rgb[2] * k * 255.0).min(255.0) as u8,
                255,
            ]
        };
        // The main channel's halo: the lit air about it, light cyan, one mitered ribbon facing the eye down the channel so
        // its glow is smooth where the channel bends.
        if main >= 0.02 {
            let pieces: Vec<&Segment> = self
                .segments
                .iter()
                .filter(|s| !s.branch && s.down <= reach)
                .collect();
            if let Some(first) = pieces.first() {
                let points: Vec<Vec3> = std::iter::once(first.a)
                    .chain(pieces.iter().map(|s| s.b))
                    .collect();
                let toward = (EYE - self.hit).normalize_or(-Vec3::Y);
                let color = scale([0.5, 0.9, 1.0], main * 0.55);
                ribbon(&points, &vec![color; points.len()], 0.6, 0.0, toward, out);
            }
        }
        for s in &self.segments {
            if s.down > reach {
                continue;
            }
            let k = if s.branch { branches } else { main };
            if k < 0.02 {
                continue;
            }
            // A branch's halo, thin; the core: white-hot, drawn twice when the stroke is on so it burns past the sprite's
            // range.
            if s.branch {
                beam(
                    s.a,
                    s.b,
                    0.5 * s.width,
                    scale([0.5, 0.9, 1.0], k * 0.5),
                    EYE,
                    out,
                );
            }
            beam(
                s.a,
                s.b,
                0.09 * s.width,
                scale([1.0, 1.0, 1.0], k),
                EYE,
                out,
            );
            if k > 1.0 {
                beam(s.a, s.b, 0.07 * s.width, scale([1.0; 3], k - 1.0), EYE, out);
            }
        }
    }
}

pub struct Storm {
    nodes: Vec<Node>,
    rain: usize,
    sparks: Grit,
    flash: usize,
    bolts: Vec<Bolt>,
    /// When the next strike comes, and how many have struck (their seeds).
    next: f32,
    struck: u32,
    /// Where the sheet lightning flickers inside the cloud, from when.
    sheet: Option<(Vec3, f32)>,
    next_sheet: f32,
    debt: f32,
}

impl Storm {
    pub fn new() -> Storm {
        let drop = ParticleData {
            lifetime_ms: (1000.0 * RAIN.4 / RAIN_SPEED) as i32 + 10,
            inv_alpha: true,
            colors: [[0.75, 0.8, 0.9, 0.55]; 4],
            sizes: [0.05; 4],
            ..particle(DROP)
        };
        // The sparks where the bolt hits: white-hot, flying out and bouncing on the ground, cooling as they lie.
        let sparks = Grit::new(
            GritKind {
                gravity: 9.81,
                drag: 0.3,
                elasticity: 0.4,
                friction: 0.3,
                tail: 0.03,
                size: 0.04,
                life: (0.5, 1.3),
                colors: [
                    [1.0, 1.0, 1.0, 1.0],
                    [0.8, 0.9, 1.0, 1.0],
                    [1.0, 0.7, 0.3, 0.7],
                    [0.6, 0.2, 0.05, 0.0],
                ],
                times: [0.0, 0.1, 0.5, 1.0],
            },
            0.0,
        );
        let flash = ParticleData {
            lifetime_ms: 140,
            colors: [
                [0.9, 0.95, 1.0, 0.9],
                [0.7, 0.8, 1.0, 0.5],
                [0.5, 0.6, 1.0, 0.2],
                [0.4, 0.5, 1.0, 0.0],
            ],
            sizes: [0.6, 1.1, 1.4, 1.5],
            ..particle(GLOW)
        };
        let mut nodes = Vec::new();
        let rain = one(
            &mut nodes,
            EmitterData {
                velocity: RAIN_SPEED,
                velocity_variance: 0.0,
                theta_max: 0.0,
                ..emitter()
            },
            vec![(
                drop,
                ParticleExtras {
                    size_by_velocity: 0.03,
                    collide: true,
                    bounce_elasticity: 0.0,
                    bounce_friction: 1.0,
                    ..ParticleExtras::default()
                },
            )],
        );
        let flash = one(
            &mut nodes,
            EmitterData {
                velocity: 0.0,
                velocity_variance: 0.0,
                ..emitter()
            },
            vec![plain(flash)],
        );
        Storm {
            nodes,
            rain,
            sparks,
            flash,
            bolts: Vec::new(),
            next: 1.2,
            struck: 0,
            sheet: None,
            next_sheet: 0.4,
            debt: 0.0,
        }
    }

    /// The cloud deck: a lumpy dark sheet at the cloud base, lit (the flashes light its underside), and the field's trees.
    fn scenery() -> Vec<DynamicObject> {
        const NX: usize = 36;
        const NY: usize = 34;
        let (x0, x1, y0, y1) = DECK;
        // Smooth value noise over the deck: the lumps hang lower where it is dense.
        let noise = |x: f32, y: f32| -> f32 {
            let (fx, fy) = (x / 9.0, y / 9.0);
            let (ix, iy) = (fx.floor(), fy.floor());
            let (tx, ty) = (fx - ix, fy - iy);
            let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
            let at = |i: f32, j: f32| {
                hash((i as i32 as u32).wrapping_mul(73) ^ (j as i32 as u32).wrapping_mul(1543))
            };
            let top = at(ix, iy) * (1.0 - sx) + at(ix + 1.0, iy) * sx;
            let bottom = at(ix, iy + 1.0) * (1.0 - sx) + at(ix + 1.0, iy + 1.0) * sx;
            top * (1.0 - sy) + bottom * sy
        };
        let height = |x: f32, y: f32| -> f32 {
            let n = noise(x, y) * 0.7 + noise(x * 2.3 + 40.0, y * 2.3 + 17.0) * 0.3;
            CLOUD_BASE + 4.0 - 6.0 * n
        };
        let at = |i: usize, j: usize| {
            let x = x0 + (x1 - x0) * i as f32 / NX as f32;
            let y = y0 + (y1 - y0) * j as f32 / NY as f32;
            Vec3::new(x, y, height(x, y))
        };
        let shade = |p: Vec3| {
            let low = ((CLOUD_BASE + 4.0 - p.z) / 6.0).clamp(0.0, 1.0);
            let v = 52.0 - 22.0 * low;
            [v as u8, (v * 1.03) as u8, (v * 1.12) as u8, 255]
        };
        let mut deck = Vec::with_capacity(NX * NY * 6);
        for i in 0..NX {
            for j in 0..NY {
                let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                let n = -(b - a).cross(d - a).normalize_or(Vec3::Z);
                let n = if n.z > 0.0 { -n } else { n };
                deck.extend([
                    vertex(a, n, shade(a), [0.0; 2]),
                    vertex(b, n, shade(b), [0.0; 2]),
                    vertex(c, n, shade(c), [0.0; 2]),
                    vertex(a, n, shade(a), [0.0; 2]),
                    vertex(c, n, shade(c), [0.0; 2]),
                    vertex(d, n, shade(d), [0.0; 2]),
                ]);
            }
        }
        let mut trees = Vec::new();
        // The dead tree: a trunk and two bare limbs.
        cuboid(
            TREE + Vec3::Z * TREE_HEIGHT * 0.5,
            Vec3::new(0.22, 0.22, TREE_HEIGHT * 0.5),
            [30, 26, 22],
            &mut trees,
        );
        scenery::log(
            TREE + Vec3::Z * 4.5,
            TREE + Vec3::new(-1.8, 0.4, 6.8),
            0.09,
            [30, 26, 22],
            &mut trees,
        );
        scenery::log(
            TREE + Vec3::Z * 5.4,
            TREE + Vec3::new(1.4, -0.6, 7.2),
            0.08,
            [30, 26, 22],
            &mut trees,
        );
        let mut g = Lcg(29);
        for (x, y) in [
            (-16.0, 26.0),
            (-9.0, 31.0),
            (12.0, 29.0),
            (19.0, 24.0),
            (-22.0, 18.0),
            (24.0, 33.0),
        ] {
            let h = g.range(3.0, 4.5);
            let foot = Vec3::new(x, y, 0.0);
            cuboid(
                foot + Vec3::Z * h * 0.5,
                Vec3::new(0.2, 0.2, h * 0.5),
                [18, 16, 14],
                &mut trees,
            );
            for _ in 0..3 {
                let off = Vec3::new(g.range(-1.2, 1.2), g.range(-0.8, 0.8), g.range(-0.5, 1.0));
                let r = g.range(1.6, 2.6);
                canopy(
                    foot + Vec3::Z * h + off,
                    Vec3::new(r, r, r * 0.8),
                    g.0,
                    [22, 30, 20],
                    &mut trees,
                );
            }
        }
        let mut blades = Vec::new();
        scenery::blades(31, 5000, -14.0..14.0, -10.0..24.0, &mut blades);
        vec![
            scenery::mesh(320, "cloud deck", deck, false, None),
            scenery::mesh(321, "trees", trees, false, None),
            scenery::mesh(322, "grass", blades, false, None),
        ]
    }

    fn strike(&mut self, t: f32, hit: Vec3) {
        self.struck += 1;
        self.bolts
            .push(Bolt::new(t, hit, 0x5a17 + u64::from(self.struck) * 7919));
    }

    /// Where the next bolt goes: the dead tree's top a third of the time, else the open field.
    fn target(&self, rng: &mut Rng) -> Vec3 {
        if rng.f() < 0.35 {
            TREE + Vec3::Z * TREE_HEIGHT
        } else {
            Vec3::new(-12.0 + 24.0 * rng.f(), 4.0 + 26.0 * rng.f(), 0.0)
        }
    }
}

impl Effect for Storm {
    fn setting(&self) -> Setting {
        Setting {
            sun: ([-0.2, 0.5, 0.84], [0.0; 3], [0.005, 0.006, 0.011]),
            clear: [0.004, 0.005, 0.009],
            scenery: Storm::scenery(),
            floor: Floor::Grass,
            ..Setting::new(EYE.to_array(), [0.5, 12.0, 6.5], 62)
        }
    }

    fn step(&mut self, t: f32, ms: u32, rng: &mut Rng) {
        let dt = ms as f32 / 1000.0;
        self.debt += RAIN_RATE * dt;
        let (x0, x1, y0, y1, h) = RAIN;
        let lean = Vec3::new(0.12, 0.06, -1.0).normalize();
        while self.debt >= 1.0 {
            self.debt -= 1.0;
            let at = Vec3::new(x0 + (x1 - x0) * rng.f(), y0 + (y1 - y0) * rng.f(), h);
            self.nodes[self.rain].burst(at, lean, 1, rng);
        }
        if t >= self.next {
            let hit = self.target(rng);
            self.strike(t, hit);
            self.next = t + 2.4 + 2.6 * rng.f();
        }
        if t >= self.next_sheet {
            let at = Vec3::new(
                -30.0 + 60.0 * rng.f(),
                30.0 + 40.0 * rng.f(),
                CLOUD_BASE - 1.0,
            );
            self.sheet = Some((at, t));
            self.next_sheet = t + 1.5 + 3.0 * rng.f();
        }
        // The return stroke: sparks and a flash where the bolt hit.
        for k in 0..self.bolts.len() {
            let b = &self.bolts[k];
            let (before, now) = (t - dt - b.t0, t - b.t0);
            if before < STROKE_AT && now >= STROKE_AT {
                let hit = b.hit;
                let (speed, spread, cone) = SPARKS;
                for _ in 0..180 {
                    let v =
                        within_cone(Vec3::Z, cone, rng) * (speed + spread * (rng.f() * 2.0 - 1.0));
                    self.sparks.throw(hit + Vec3::Z * 0.1, v, rng);
                }
                self.nodes[self.flash].burst(hit + Vec3::Z * 0.6, Vec3::Z, 1, rng);
            }
        }
        self.sparks.step(dt);
        self.bolts.retain(|b| t - b.t0 < STRIKE_S);
    }

    fn nodes(&mut self) -> &mut [Node] {
        &mut self.nodes
    }

    fn trigger(&mut self, t: f32, rng: &mut Rng) {
        let hit = self.target(rng);
        self.strike(t, hit);
        self.next = self.next.max(t + 2.0);
    }

    fn lights(&self, t: f32, frame: u32) -> Vec<HdLight> {
        let mut out = Vec::new();
        for b in &self.bolts {
            let a = t - b.t0;
            let (main, _, _) = Bolt::glow(a);
            if a < STROKE_AT {
                // The leader's faint light high up.
                out.push(light(
                    Vec3::new(b.hit.x, b.hit.y, CLOUD_BASE - 3.0),
                    [0.6, 0.85, 1.0],
                    3.0,
                    60.0,
                ));
                continue;
            }
            let flash = main.min(STROKE) / STROKE;
            // The whole scene: a light halfway down the channel; the ground: one at the hit; the cloud base: one under it.
            out.push(light(
                Vec3::new(b.hit.x, b.hit.y, CLOUD_BASE * 0.55),
                [0.72, 0.9, 1.0],
                220.0 * flash,
                140.0,
            ));
            out.push(light(
                b.hit + Vec3::Z * 1.5,
                [0.85, 0.95, 1.0],
                40.0 * flash,
                36.0,
            ));
            out.push(light(
                Vec3::new(b.hit.x, b.hit.y, CLOUD_BASE - 2.5),
                [0.72, 0.9, 1.0],
                110.0 * flash,
                90.0,
            ));
        }
        if let Some((at, t0)) = self.sheet {
            let a = t - t0;
            if a < 0.4 {
                let k = (1.0 - a / 0.4) * (0.5 + 0.5 * hash(frame));
                out.push(light(at, [0.6, 0.7, 1.0], 25.0 * k, 70.0));
            }
        }
        out
    }

    /// The stroke's flash over the whole scene: the brightest bolt's share of the stroke, the sheet lightning a little.
    fn flash(&self, t: f32) -> f32 {
        let bolts = self
            .bolts
            .iter()
            .map(|b| {
                let a = t - b.t0;
                if a < STROKE_AT {
                    0.0
                } else {
                    (Bolt::glow(a).0 / STROKE).min(1.0)
                }
            })
            .fold(0.0, f32::max);
        let sheet = self.sheet.map_or(0.0, |(_, t0)| {
            let a = t - t0;
            if a < 0.4 { 0.12 * (1.0 - a / 0.4) } else { 0.0 }
        });
        bolts.max(sheet)
    }

    fn decals(&self, t: f32, textures: &mut TextureSet<'_>) -> Vec<(f32, Draw)> {
        let mut glow = Vec::new();
        for b in &self.bolts {
            b.shape(t, &mut glow);
        }
        let mut sparks = Vec::with_capacity(self.sparks.len() * 6);
        self.sparks.shape(EYE, &mut sparks);
        let line = textures.get(LINE);
        vec![
            (0.0, scenery::decal("bolts", glow, line)),
            (0.0, scenery::decal("sparks", sparks, line)),
        ]
    }
}
