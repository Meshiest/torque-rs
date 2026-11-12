//! Grit the script moves itself: hot spatter and sparks that fly on ballistic arcs, strike the ground, bounce, slide and
//! come to rest, cooling where they lie. The pool's particles know nothing of the floor, so these are stepped here and drawn
//! each frame as glowing streaks stretched along their motion (a decal of camera-facing quads with the line texture across
//! them).

use glam::Vec3;
use torque_render_scene::Vertex;
use torque_render_scene::particles::Rng;

use crate::scenery::beam;

/// How grains of one kind fly and look.
#[derive(Clone, Copy)]
pub struct GritKind {
    /// Units a second squared, down.
    pub gravity: f32,
    /// The share of a grain's velocity lost a second.
    pub drag: f32,
    /// The share of a grain's speed into the ground kept at a bounce, and the share of its speed along it lost.
    pub elasticity: f32,
    pub friction: f32,
    /// The streak's tail: seconds of the grain's speed behind it.
    pub tail: f32,
    /// A grain's width (units); a grain at rest is drawn this long too.
    pub size: f32,
    /// Seconds a grain lives, least and most.
    pub life: (f32, f32),
    /// The grain's color over its life (a datablock's keys; the alpha is its brightness) at `times` (0 to 1).
    pub colors: [[f32; 4]; 4],
    pub times: [f32; 4],
}

struct Grain {
    pos: Vec3,
    vel: Vec3,
    age: f32,
    life: f32,
    rest: bool,
}

/// A bounce slower than this (units a second) up off the ground leaves the grain at rest.
const REST_SPEED: f32 = 0.25;

pub struct Grit {
    kind: GritKind,
    /// The ground's height the grains land on.
    floor: f32,
    grains: Vec<Grain>,
}

impl Grit {
    pub fn new(kind: GritKind, floor: f32) -> Grit {
        Grit {
            kind,
            floor,
            grains: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.grains.len()
    }

    /// One grain thrown from `at` with velocity `vel`.
    pub fn throw(&mut self, at: Vec3, vel: Vec3, rng: &mut Rng) {
        let (lo, hi) = self.kind.life;
        self.grains.push(Grain {
            pos: at,
            vel,
            age: 0.0,
            life: lo + (hi - lo) * rng.f(),
            rest: false,
        });
    }

    /// `dt` seconds of flight, bounces and rest; the grains past their life gone.
    pub fn step(&mut self, dt: f32) {
        let k = self.kind;
        let ground = self.floor + k.size * 0.5;
        for g in &mut self.grains {
            g.age += dt;
            if g.rest {
                continue;
            }
            g.vel += (Vec3::NEG_Z * k.gravity - g.vel * k.drag) * dt;
            g.pos += g.vel * dt;
            if g.pos.z >= ground {
                continue;
            }
            g.pos.z = ground;
            if g.vel.z >= 0.0 {
                continue;
            }
            let up = -g.vel.z * k.elasticity;
            g.vel = Vec3::new(
                g.vel.x * (1.0 - k.friction),
                g.vel.y * (1.0 - k.friction),
                up,
            );
            if up < REST_SPEED {
                g.vel = Vec3::ZERO;
                g.rest = true;
            }
        }
        self.grains.retain(|g| g.age < g.life);
    }

    /// The grains as streaks facing `eye`, appended to `out`.
    pub fn shape(&self, eye: Vec3, out: &mut Vec<Vertex>) {
        let k = self.kind;
        for g in &self.grains {
            let c = keyed(&k.colors, &k.times, g.age / g.life);
            if c[3] < 0.01 {
                continue;
            }
            let color = [
                (c[0] * c[3] * 255.0).min(255.0) as u8,
                (c[1] * c[3] * 255.0).min(255.0) as u8,
                (c[2] * c[3] * 255.0).min(255.0) as u8,
                255,
            ];
            let tail = g.vel * k.tail;
            let tail = if tail.length() < k.size {
                // At rest or slow: a square of the grain's size (facing the eye, so it reads as a point).
                tail.normalize_or(Vec3::Z) * k.size
            } else {
                tail
            };
            beam(g.pos - tail, g.pos, k.size, color, eye, out);
        }
    }
}

/// The color keys at `t` (0 to 1) of a life, as the pool interpolates a datablock's.
fn keyed(colors: &[[f32; 4]; 4], times: &[f32; 4], t: f32) -> [f32; 4] {
    let k = times.iter().position(|&time| time >= t).unwrap_or(4);
    if k == 0 {
        return colors[0];
    }
    if k >= 4 {
        return colors[3];
    }
    let span = times[k] - times[k - 1];
    let f = if span > 0.0 {
        ((t - times[k - 1]) / span).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let mut c = [0.0; 4];
    for (i, v) in c.iter_mut().enumerate() {
        *v = colors[k - 1][i] * (1.0 - f) + colors[k][i] * f;
    }
    c
}
