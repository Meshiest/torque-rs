//! A spiral galaxy of a quarter of a million stars, each a crisp point, seen from a camera on a slow orbit above its disk.
//! Stars are born where the galaxy is dense (a bright bulge, two strong arms and two weaker ones between them, a thin disk
//! of old stars between the arms) and move on their orbits: the bulge turns as one body, the disk outside it at one speed,
//! so the inner parts turn through more of their orbit than the outer (a flat rotation curve). Each star lives a few
//! seconds and fades in and out, so the arms, a standing pattern, stay where stars keep being born on them while the stars
//! themselves drift through. Young blue stars line the arms, with pink knots of glowing gas; the arms' inner edges carry a
//! dark lane where no stars are born; the bulge is yellow-white and the disk between the arms yellow; a few red giants
//! are scattered through. The galaxy's pattern turns slowly too, and the camera orbits the other way.

use std::f32::consts::TAU;

use glam::Vec3;
use torque_render_scene::hd::HdLight;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::particles::{EmitterData, ParticleData, ParticleExtras, Rng};
use torque_render_scene::source::Eye;

use super::{Effect, Lcg, Node, Setting, emitter, one, particle, plain};
use crate::scenery::{self, cuboid};
use crate::sprites::{GLOW, MOTE, SPARK};

/// The galaxy's center: high over the stage, so the stage's floor is out of sight below the frame.
const CENTER: Vec3 = Vec3::new(0.0, 0.0, 70.0);
/// The disk's radius and its scale length (stars thin out as exp(-r / R_D)), the bulge's radius and share of the stars.
const R_DISK: f32 = 19.0;
const R_D: f32 = 5.5;
const R_BULGE: f32 = 3.2;
const BULGE: f32 = 0.2;
const HALO: f32 = 0.025;
/// The arms: logarithmic spirals of this pitch (radians) through `R_0` at angle 0; the share of disk stars between the
/// arms rather than on them.
const PITCH: f32 = 0.24;
const R_0: f32 = 0.8;
const FIELD: f32 = 0.3;
/// The disk's rotation: one speed outside `R_CORE` (units a second), rising from the center inside it; and how fast the
/// arms' pattern itself turns (radians a second).
const V_FLAT: f32 = 0.5;
const R_CORE: f32 = 3.0;
const PATTERN: f32 = 0.012;
/// Stars born a second, and how long one shows.
const RATE: f32 = 58_000.0;
const LIFE_MS: i32 = 4000;
/// The camera's orbit: its distance, how fast it goes around, and its height over the disk's plane (an angle that breathes).
const ORBIT_R: f32 = 44.0;
const ORBIT_W: f32 = -0.035;
const TILT: f32 = 0.5;

/// A star's kind, each with its color and size: the bulge's yellow-white, the disk's yellow, the arms' white and young
/// blue, the red giants; and the pink knots of gas on the arms.
#[derive(Clone, Copy)]
enum Kind {
    Bulge,
    Old,
    White,
    Blue,
    Red,
    Knot,
}

const KINDS: [Kind; 6] = [
    Kind::Bulge,
    Kind::Old,
    Kind::White,
    Kind::Blue,
    Kind::Red,
    Kind::Knot,
];

impl Kind {
    /// Its datablock: the color (additive, fading in and out over the life) and the point's size.
    fn data(self) -> (ParticleData, ParticleExtras) {
        let (color, size, texture, life) = match self {
            Kind::Bulge => ([1.0, 0.92, 0.75], 0.075, SPARK, LIFE_MS),
            Kind::Old => ([1.0, 0.86, 0.6], 0.075, SPARK, LIFE_MS),
            Kind::White => ([0.92, 0.94, 1.0], 0.08, SPARK, LIFE_MS),
            Kind::Blue => ([0.55, 0.7, 1.0], 0.1, SPARK, LIFE_MS),
            Kind::Red => ([1.0, 0.5, 0.3], 0.14, SPARK, LIFE_MS),
            Kind::Knot => ([1.0, 0.45, 0.6], 0.6, MOTE, LIFE_MS * 2),
        };
        let [r, g, b] = color;
        let peak = match self {
            Kind::Knot => 0.35,
            Kind::Bulge => 0.6,
            _ => 1.0,
        };
        let p = ParticleData {
            lifetime_ms: life,
            lifetime_variance_ms: life / 3,
            colors: [
                [r, g, b, 0.0],
                [r, g, b, peak],
                [r, g, b, peak],
                [r, g, b, 0.0],
            ],
            sizes: [size; 4],
            times: [0.0, 0.2, 0.8, 1.0],
            ..particle(texture)
        };
        plain(p)
    }
}

/// A standard normal from two uniforms (Box-Muller). The showcase's own generator places the stars: the pool's, a
/// minimal-standard one, lays consecutive draws on planes, which showed as rings in the bulge.
fn gaussian(g: &mut Lcg) -> f32 {
    let u = g.unit().max(1e-6);
    let v = g.unit();
    (-2.0 * u.ln()).sqrt() * (TAU * v).cos()
}

/// The arm `k` (0..4)'s angle at radius `r`: the strong pair a half turn apart, the weak pair between them, each
/// wandering a little off its spiral so the four are not one shape turned.
fn arm_angle(r: f32, k: usize) -> f32 {
    let wander = 0.14 * (0.55 * r + 1.7 * k as f32).sin() + 0.08 * (1.3 * r + 0.9 * k as f32).cos();
    (r.max(R_0) / R_0).ln() / PITCH.tan() + TAU * k as f32 / 4.0 + wander
}

/// A star's orbital speed at `r`: solid-body rotation in the core, flat outside it.
fn orbit_speed(r: f32) -> f32 {
    V_FLAT * (r / R_CORE).min(1.0)
}

/// A star to be born: where, its kind, and its orbit's direction.
struct Star {
    at: Vec3,
    kind: Kind,
    along: Vec3,
}

pub struct Galaxy {
    nodes: Vec<Node>,
    kinds: [usize; KINDS.len()],
    glow: usize,
    /// The births owed and not yet made (a fraction of one).
    owed: f32,
    lcg: Lcg,
}

impl Galaxy {
    pub fn new() -> Galaxy {
        let still = EmitterData {
            velocity: 0.0,
            velocity_variance: 0.0,
            theta_max: 0.0,
            ..emitter()
        };
        let mut nodes = Vec::new();
        let kinds = KINDS.map(|k| one(&mut nodes, still.clone(), vec![k.data()]));
        // The bulge's own light: wide warm glows over the center, each alive two seconds, made over and over.
        let glow = one(
            &mut nodes,
            still,
            vec![plain(ParticleData {
                lifetime_ms: 2000,
                colors: [
                    [1.0, 0.95, 0.85, 0.0],
                    [1.0, 0.95, 0.85, 0.12],
                    [1.0, 0.95, 0.85, 0.12],
                    [1.0, 0.95, 0.85, 0.0],
                ],
                sizes: [5.0; 4],
                times: [0.0, 0.3, 0.7, 1.0],
                ..particle(GLOW)
            })],
        );
        Galaxy {
            nodes,
            kinds,
            glow,
            owed: 0.0,
            lcg: Lcg(0x6761_6c61),
        }
    }

    /// A star in the bulge: a steep profile toward the center, a little flattened.
    fn bulge(g: &mut Lcg) -> Vec3 {
        let r = R_BULGE * g.unit().powf(2.4) + 0.05;
        let z = g.unit() * 2.0 - 1.0;
        let a = g.unit() * TAU;
        let s = (1.0 - z * z).sqrt();
        Vec3::new(s * a.cos(), s * a.sin(), z * 0.65) * r
    }

    /// A star of the disk at pattern angle `turn`: on an arm (a Gaussian spread across it that widens outward, the young
    /// stars at its heart, a dust lane cut from its inner edge) or in the field between the arms. None: swallowed by the
    /// dust lane.
    fn disk(turn: f32, g: &mut Lcg) -> Option<(Vec3, Kind)> {
        // The radius from the exponential disk, cut at its edge.
        let edge = 1.0 - (-R_DISK / R_D).exp();
        let r = -R_D * (1.0 - g.unit() * edge).ln();
        let r = r.max(0.3);
        let thickness = 0.1 + 0.012 * r;
        let z = gaussian(g) * thickness;
        // Inside the bulge the arms have not formed: the disk there is all field.
        let field = FIELD + (1.0 - FIELD) * (1.0 - super::smooth((r - 1.5) / 2.5));
        if g.unit() < field {
            let theta = g.unit() * TAU + turn;
            let kind = if g.unit() < 0.012 {
                Kind::Red
            } else {
                Kind::Old
            };
            return Some((Vec3::new(r * theta.cos(), r * theta.sin(), z), kind));
        }
        // The strong arms take twice the stars of the weak ones.
        let pick = g.unit() * 3.0;
        let k = if pick < 1.0 {
            0
        } else if pick < 2.0 {
            2
        } else if pick < 2.5 {
            1
        } else {
            3
        };
        let sigma = 0.3 + 0.07 * r;
        let d = gaussian(g) * sigma;
        // The dust lane: a band just inside the arm's center swallows most of what is born there.
        let lane = ((d + 0.9 * sigma) / (0.3 * sigma)).powi(2);
        if (-lane).exp() * 0.85 > g.unit() {
            return None;
        }
        let theta = arm_angle(r, k) + d / r + turn;
        let young = d.abs() < 0.45 * sigma;
        let roll = g.unit();
        let kind = if roll < 0.004 && young {
            Kind::Knot
        } else if roll < 0.012 {
            Kind::Red
        } else if young && roll < 0.65 {
            Kind::Blue
        } else if r < 5.0 && roll < 0.8 {
            Kind::Bulge
        } else if roll < 0.75 {
            Kind::White
        } else {
            Kind::Old
        };
        Some((Vec3::new(r * theta.cos(), r * theta.sin(), z), kind))
    }

    fn star(turn: f32, g: &mut Lcg) -> Option<Star> {
        let pick = g.unit();
        let (offset, kind) = if pick < BULGE {
            (Galaxy::bulge(g), Kind::Bulge)
        } else if pick < BULGE + HALO {
            // The halo: a thin sphere of old stars about the whole.
            let dir = Vec3::new(gaussian(g), gaussian(g), gaussian(g)).normalize_or(Vec3::Z);
            (
                dir * (R_BULGE + (R_DISK + 4.0) * g.unit().powf(1.5)),
                Kind::Old,
            )
        } else {
            Galaxy::disk(turn, g)?
        };
        let r = offset.truncate().length().max(0.05);
        let along = Vec3::new(-offset.y, offset.x, 0.0) / r;
        Some(Star {
            at: CENTER + offset,
            kind,
            along,
        })
    }
}

impl Effect for Galaxy {
    fn setting(&self) -> Setting {
        // Space: a black box about the galaxy and its camera, so every star has a surface behind it (a sprite over the
        // bare sky is dropped).
        let mut void = Vec::new();
        cuboid(CENTER, Vec3::splat(52.0), [0, 0, 0], &mut void);
        Setting {
            sun: ([0.0, 0.0, 1.0], [0.0; 3], [0.0; 3]),
            clear: [0.0, 0.0, 0.002],
            scenery: vec![scenery::mesh(300, "void", void, false, None)],
            ..Setting::new(CENTER.to_array(), CENTER.to_array(), 48)
        }
    }

    fn step(&mut self, t: f32, ms: u32, rng: &mut Rng) {
        let turn = PATTERN * t;
        self.owed += RATE * ms as f32 / 1000.0;
        while self.owed >= 1.0 {
            self.owed -= 1.0;
            let Some(star) = Galaxy::star(turn, &mut self.lcg) else {
                continue;
            };
            let r = (star.at - CENTER).truncate().length();
            let node = &mut self.nodes[self.kinds[star.kind as usize]];
            node.emitter.data.velocity = orbit_speed(r) + PATTERN * r;
            node.burst(star.at, star.along, 1, rng);
        }
        // The core's glow, three sizes of it, kept up.
        if (t * 1000.0) as u32 % 400 < ms {
            for (k, size) in [10.0, 5.0, 2.4].into_iter().enumerate() {
                let n = &mut self.nodes[self.glow];
                let p = &mut n.emitter.particle_data[0];
                p.sizes = [size; 4];
                p.colors[1][3] = [0.2, 0.35, 0.6][k];
                p.colors[2][3] = p.colors[1][3];
                n.burst(CENTER, Vec3::Z, 1, rng);
            }
        }
    }

    fn nodes(&mut self) -> &mut [Node] {
        &mut self.nodes
    }

    fn lights(&self, _t: f32, _frame: u32) -> Vec<HdLight> {
        Vec::new()
    }

    /// The camera on its orbit: around the galaxy, its height over the disk breathing between a low and a high view.
    fn eye(&self, t: f32) -> Option<Eye> {
        let a = ORBIT_W * t + 0.6;
        let tilt = TILT + 0.2 * (0.07 * t).sin();
        let eye =
            CENTER + Vec3::new(a.cos() * tilt.cos(), a.sin() * tilt.cos(), tilt.sin()) * ORBIT_R;
        let mut eye = look_at(eye.to_array(), CENTER.to_array());
        eye.fov = Some(48);
        Some(eye)
    }
}
