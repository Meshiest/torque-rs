//! A fireworks show over a dark shore: shells climb on flickering trails and burst high up as peonies, chrysanthemums
//! (comets with streaks), willows (long gold hanging trails), rings, palms (thick comets), crossettes (stars that split),
//! strobes (twinkling white glitter) and a finale of many at once. Stars slow on drag and droop on gravity, go from white-hot
//! to their color and fade; each burst flashes a light over the scene and leaves a faint smoke cloud that later bursts light.

use std::f32::consts::TAU;
use std::ops::Range;

use glam::Vec3;
use torque_render_scene::hd::HdLight;
use torque_render_scene::particles::{EmitterData, ParticleData, ParticleExtras, Rng};

use super::{Effect, Floor, Node, Setting, emitter, group, light, one, particle, plain};
use crate::sprites::{GLOW, SMOKES, SPARK};

/// Where the shells leave the ground, and the camera.
const PAD_X: Range<f32> = -14.0..14.0;
const PAD_Y: Range<f32> = 24.0..40.0;
const EYE: [f32; 3] = [0.0, -24.0, 2.0];
/// The scene's gravity (units a second squared; the datablocks' `gravity` of 1 is the same).
const G: f32 = 9.81;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Peony,
    Chrysanthemum,
    Willow,
    Ring,
    Palm,
    Crossette,
    Strobe,
}

/// The show: when each shell leaves, what it is and its color (gamma 0..1).
const SHOW: [(f32, Kind, [f32; 3]); 12] = [
    (0.3, Kind::Peony, [1.0, 0.3, 0.25]),
    (1.8, Kind::Chrysanthemum, [1.0, 0.75, 0.3]),
    (3.0, Kind::Ring, [0.35, 0.6, 1.0]),
    (4.3, Kind::Willow, [1.0, 0.72, 0.3]),
    (5.6, Kind::Palm, [1.0, 0.55, 0.2]),
    (6.6, Kind::Crossette, [0.4, 1.0, 0.45]),
    (7.8, Kind::Strobe, [1.0, 1.0, 1.0]),
    (8.0, Kind::Peony, [0.75, 0.4, 1.0]),
    (9.4, Kind::Chrysanthemum, [0.4, 0.9, 1.0]),
    (10.4, Kind::Ring, [1.0, 0.4, 0.7]),
    (10.9, Kind::Peony, [1.0, 0.85, 0.4]),
    (12.0, Kind::Willow, [1.0, 0.72, 0.3]),
];
/// The finale: shells a fraction of a second apart from here on.
const FINALE_START: f32 = 13.3;
const FINALE: [(Kind, [f32; 3]); 8] = [
    (Kind::Peony, [1.0, 0.3, 0.25]),
    (Kind::Chrysanthemum, [1.0, 0.75, 0.3]),
    (Kind::Peony, [0.35, 0.6, 1.0]),
    (Kind::Palm, [1.0, 0.55, 0.2]),
    (Kind::Peony, [0.4, 1.0, 0.45]),
    (Kind::Strobe, [1.0, 1.0, 1.0]),
    (Kind::Ring, [0.75, 0.4, 1.0]),
    (Kind::Willow, [1.0, 0.72, 0.3]),
];

/// A shell on its way up: its trail emitter, where it is, when it bursts and what it bursts into.
struct Shell {
    trail: usize,
    pos: Vec3,
    vel: Vec3,
    burst_at: f32,
    kind: Kind,
    color: [f32; 3],
}

/// A burst that happened: its flash lights the scene for a moment.
struct Burst {
    at: Vec3,
    t0: f32,
    color: [f32; 3],
}

/// Something due later: a crossette star splitting where it will be, or a strobe's glitter popping.
enum Due {
    Split {
        at: Vec3,
        vel: Vec3,
        color: [f32; 3],
    },
    Glint {
        center: Vec3,
        spread: f32,
    },
}

/// A star's color keys: white-hot, the color, the color dimming, out; `alpha` the second key's.
fn star_keys(color: [f32; 3], alpha: f32) -> [[f32; 4]; 4] {
    let dim = color.map(|c| c * 0.55);
    [
        [1.0, 1.0, 0.95, 1.0],
        [color[0], color[1], color[2], alpha],
        [dim[0], dim[1], dim[2], alpha * 0.6],
        [dim[0] * 0.5, dim[1] * 0.5, dim[2] * 0.5, 0.0],
    ]
}

pub struct Fireworks {
    /// When the show's clock started (it starts over after the finale).
    t0: f32,
    nodes: Vec<Node>,
    trails: Range<usize>,
    stars: usize,
    comets: usize,
    willow: usize,
    palms: usize,
    ring: usize,
    glitter: usize,
    flash: usize,
    smoke: usize,
    shells: Vec<Shell>,
    bursts: Vec<Burst>,
    due: Vec<(f32, Due)>,
    launched: usize,
    finale_due: f32,
    finale_launched: usize,
}

impl Fireworks {
    pub fn new() -> Fireworks {
        let own = EmitterData {
            use_emitter_colors: true,
            theta_max: 180.0,
            ..emitter()
        };
        let star = ParticleData {
            drag: 1.1,
            gravity: 0.45,
            lifetime_ms: 1900,
            lifetime_variance_ms: 600,
            sizes: [0.4, 0.36, 0.28, 0.1],
            times: [0.0, 0.12, 0.6, 1.0],
            ..particle(SPARK)
        };
        let trail = ParticleData {
            drag: 0.6,
            gravity: 0.2,
            lifetime_ms: 450,
            lifetime_variance_ms: 250,
            colors: [
                [1.0, 0.85, 0.55, 1.0],
                [1.0, 0.6, 0.25, 0.8],
                [0.8, 0.3, 0.08, 0.4],
                [0.3, 0.08, 0.0, 0.0],
            ],
            sizes: [0.16, 0.12, 0.08, 0.03],
            ..particle(SPARK)
        };
        let glint = ParticleData {
            drag: 0.8,
            gravity: 0.3,
            lifetime_ms: 140,
            lifetime_variance_ms: 80,
            colors: [
                [1.0, 1.0, 1.0, 0.0],
                [1.0, 1.0, 1.0, 1.0],
                [1.0, 0.95, 0.85, 0.8],
                [1.0, 0.9, 0.7, 0.0],
            ],
            sizes: [0.2, 0.32, 0.26, 0.1],
            times: [0.0, 0.2, 0.6, 1.0],
            ..particle(SPARK)
        };
        let flash = ParticleData {
            lifetime_ms: 220,
            sizes: [2.0, 5.0, 6.0, 6.0],
            times: [0.0, 0.2, 0.6, 1.0],
            ..particle(GLOW)
        };
        let smoke = ParticleData {
            drag: 1.0,
            gravity: 0.01,
            lifetime_ms: 7000,
            lifetime_variance_ms: 2000,
            inv_alpha: true,
            spin_min: -10.0,
            spin_max: 10.0,
            colors: [
                [0.6, 0.6, 0.62, 0.0],
                [0.6, 0.6, 0.62, 0.1],
                [0.6, 0.6, 0.62, 0.07],
                [0.6, 0.6, 0.62, 0.0],
            ],
            sizes: [3.0, 7.0, 10.0, 12.0],
            times: [0.0, 0.15, 0.6, 1.0],
            ..particle(SMOKES[2])
        };
        let mut nodes = Vec::new();
        let trails = group(
            &mut nodes,
            10,
            &EmitterData {
                period_ms: 4,
                velocity: 1.5,
                velocity_variance: 1.0,
                theta_max: 25.0,
                ..emitter()
            },
            &[(
                trail,
                ParticleExtras {
                    size_by_velocity: 0.01,
                    turbulence: 1.5,
                    turbulence_scale: 0.4,
                    ..ParticleExtras::default()
                },
            )],
        );
        let stars = one(
            &mut nodes,
            EmitterData {
                velocity: 17.0,
                velocity_variance: 2.5,
                ..own.clone()
            },
            vec![(
                star.clone(),
                ParticleExtras {
                    size_by_velocity: 0.012,
                    turbulence: 0.4,
                    turbulence_scale: 2.0,
                    ..ParticleExtras::default()
                },
            )],
        );
        let comets = one(
            &mut nodes,
            EmitterData {
                velocity: 16.0,
                velocity_variance: 3.0,
                ..own.clone()
            },
            vec![(
                ParticleData {
                    drag: 1.0,
                    gravity: 0.55,
                    lifetime_ms: 2300,
                    lifetime_variance_ms: 600,
                    sizes: [0.3, 0.26, 0.2, 0.08],
                    ..star.clone()
                },
                ParticleExtras {
                    size_by_velocity: 0.09,
                    turbulence: 0.5,
                    turbulence_scale: 2.0,
                    ..ParticleExtras::default()
                },
            )],
        );
        let willow = one(
            &mut nodes,
            EmitterData {
                velocity: 13.0,
                velocity_variance: 3.0,
                ..own.clone()
            },
            vec![(
                ParticleData {
                    drag: 0.55,
                    gravity: 0.9,
                    lifetime_ms: 3600,
                    lifetime_variance_ms: 900,
                    sizes: [0.24, 0.22, 0.18, 0.06],
                    times: [0.0, 0.1, 0.7, 1.0],
                    ..star.clone()
                },
                ParticleExtras {
                    size_by_velocity: 0.16,
                    turbulence: 0.3,
                    turbulence_scale: 3.0,
                    ..ParticleExtras::default()
                },
            )],
        );
        let palms = one(
            &mut nodes,
            EmitterData {
                velocity: 15.0,
                velocity_variance: 1.5,
                ..own.clone()
            },
            vec![(
                ParticleData {
                    drag: 0.8,
                    gravity: 0.7,
                    lifetime_ms: 2600,
                    lifetime_variance_ms: 400,
                    sizes: [0.6, 0.55, 0.4, 0.12],
                    ..star.clone()
                },
                ParticleExtras {
                    size_by_velocity: 0.12,
                    turbulence: 0.6,
                    turbulence_scale: 1.5,
                    ..ParticleExtras::default()
                },
            )],
        );
        let ring = one(
            &mut nodes,
            EmitterData {
                velocity: 15.0,
                velocity_variance: 0.6,
                theta_max: 2.0,
                ..own.clone()
            },
            vec![(
                ParticleData {
                    lifetime_ms: 2000,
                    lifetime_variance_ms: 300,
                    ..star.clone()
                },
                ParticleExtras {
                    size_by_velocity: 0.012,
                    ..ParticleExtras::default()
                },
            )],
        );
        let glitter = one(
            &mut nodes,
            EmitterData {
                velocity: 1.0,
                velocity_variance: 0.8,
                theta_max: 180.0,
                ..emitter()
            },
            vec![plain(glint)],
        );
        let flash = one(
            &mut nodes,
            EmitterData {
                velocity: 0.0,
                velocity_variance: 0.0,
                ..own
            },
            vec![plain(flash)],
        );
        let smoke = one(
            &mut nodes,
            EmitterData {
                velocity: 2.5,
                velocity_variance: 1.5,
                theta_max: 180.0,
                ..emitter()
            },
            vec![(
                smoke,
                ParticleExtras {
                    turbulence: 0.4,
                    turbulence_scale: 6.0,
                    ..ParticleExtras::default()
                },
            )],
        );
        Fireworks {
            t0: 0.0,
            nodes,
            trails,
            stars,
            comets,
            willow,
            palms,
            ring,
            glitter,
            flash,
            smoke,
            shells: Vec::new(),
            bursts: Vec::new(),
            due: Vec::new(),
            launched: 0,
            finale_due: FINALE_START,
            finale_launched: 0,
        }
    }

    /// A shell leaves a pad: up and a little across, to burst near its apex.
    fn launch(&mut self, t: f32, kind: Kind, color: [f32; 3], rng: &mut Rng) {
        let used: Vec<usize> = self.shells.iter().map(|s| s.trail).collect();
        let Some(trail) = self.trails.clone().find(|i| !used.contains(i)) else {
            return;
        };
        let pos = Vec3::new(
            PAD_X.start + rng.f() * (PAD_X.end - PAD_X.start),
            PAD_Y.start + rng.f() * (PAD_Y.end - PAD_Y.start),
            0.0,
        );
        let up = 27.0 + 9.0 * rng.f();
        let vel = Vec3::new((rng.f() - 0.5) * 5.0, (rng.f() - 0.5) * 3.0, up);
        // The apex is at up / G seconds; the fuse is cut a little short of it or past it.
        let burst_at = t + up / G * (0.88 + 0.2 * rng.f());
        self.nodes[trail].emitter.break_path();
        self.shells.push(Shell {
            trail,
            pos,
            vel,
            burst_at,
            kind,
            color,
        });
    }

    fn burst(&mut self, t: f32, at: Vec3, kind: Kind, color: [f32; 3], rng: &mut Rng) {
        let keys = star_keys(color, 1.0);
        match kind {
            Kind::Peony => {
                self.nodes[self.stars].emitter.set_colors(keys, false);
                self.nodes[self.stars].burst(at, Vec3::Z, 420, rng);
            }
            Kind::Chrysanthemum => {
                self.nodes[self.comets].emitter.set_colors(keys, false);
                self.nodes[self.comets].burst(at, Vec3::Z, 300, rng);
                self.nodes[self.stars]
                    .emitter
                    .set_colors(star_keys([1.0, 1.0, 0.9], 0.9), false);
                self.nodes[self.stars].burst(at, Vec3::Z, 120, rng);
            }
            Kind::Willow => {
                self.nodes[self.willow].emitter.set_colors(keys, false);
                self.nodes[self.willow].burst(at, Vec3::Z, 380, rng);
            }
            Kind::Ring => {
                // Stars thrown evenly around a circle in a tilted plane.
                let tilt = rng.f() * 0.9 - 0.45;
                let swing = rng.f() * TAU;
                let n = 140;
                self.nodes[self.ring].emitter.set_colors(keys, false);
                for k in 0..n {
                    let a = TAU * k as f32 / n as f32;
                    let flat = Vec3::new(a.cos(), a.sin(), 0.0);
                    let dir = Vec3::new(
                        flat.x * swing.cos() - flat.y * swing.sin() * tilt.cos(),
                        flat.x * swing.sin() + flat.y * swing.cos() * tilt.cos(),
                        flat.y * tilt.sin(),
                    )
                    .normalize_or(Vec3::X);
                    self.nodes[self.ring].burst(at, dir, 1, rng);
                }
            }
            Kind::Palm => {
                self.nodes[self.palms].emitter.set_colors(keys, false);
                self.nodes[self.palms].burst(at, Vec3::Z, 16, rng);
                self.nodes[self.stars]
                    .emitter
                    .set_colors(star_keys(color, 0.7), false);
                self.nodes[self.stars].burst(at, Vec3::Z, 90, rng);
            }
            Kind::Crossette => {
                // Stars that fly out for most of a second, then each splits into four.
                self.nodes[self.stars].emitter.set_colors(keys, false);
                for _ in 0..42 {
                    let a = rng.f() * TAU;
                    let z = rng.f() * 2.0 - 1.0;
                    let r = (1.0 - z * z).sqrt();
                    let dir = Vec3::new(r * a.cos(), r * a.sin(), z);
                    let vel = dir * (12.0 + 3.0 * rng.f());
                    self.nodes[self.stars].burst(at, dir, 1, rng);
                    self.due.push((t + 0.8, Due::Split { at, vel, color }));
                }
            }
            Kind::Strobe => {
                self.nodes[self.stars]
                    .emitter
                    .set_colors(star_keys([1.0, 0.95, 0.8], 0.5), false);
                self.nodes[self.stars].burst(at, Vec3::Z, 80, rng);
                for k in 0..90 {
                    let when = t + 0.2 + 2.6 * (k as f32 / 90.0).powf(0.8);
                    self.due.push((
                        when,
                        Due::Glint {
                            center: at,
                            spread: 1.0,
                        },
                    ));
                }
            }
        }
        self.nodes[self.flash]
            .emitter
            .set_colors([[color[0], color[1], color[2], 0.8]; 4], false);
        self.nodes[self.flash].burst(at, Vec3::Z, 1, rng);
        self.nodes[self.smoke].burst(at, Vec3::Z, 22, rng);
        self.bursts.push(Burst { at, t0: t, color });
    }

    /// Where a star thrown from `at` at `vel` is after `s` seconds (the pool's step: drag and gravity, no turbulence).
    fn star_after(at: Vec3, mut vel: Vec3, s: f32) -> Vec3 {
        let mut p = at;
        let (drag, gravity) = (1.1, 0.45 * G);
        let dt = 1.0 / 60.0;
        let mut left = s;
        while left > 0.0 {
            let h = left.min(dt);
            vel += (-vel * drag - Vec3::Z * gravity) * h;
            p += vel * h;
            left -= h;
        }
        p
    }
}

impl Effect for Fireworks {
    fn setting(&self) -> Setting {
        Setting {
            sun: (
                [-0.3, 0.2, 0.93],
                [0.012, 0.014, 0.022],
                [0.012, 0.014, 0.024],
            ),
            clear: [0.004, 0.005, 0.012],
            // A dark shore across the water behind the pads.
            props: vec![([0.0, 110.0, 2.5], [160.0, 20.0, 2.5], [8, 9, 10])],
            floor: Floor::Grass,
            ..Setting::new(EYE, [0.0, 30.0, 30.0], 62)
        }
    }

    fn step(&mut self, t: f32, ms: u32, rng: &mut Rng) {
        if t - self.t0 > FINALE_START + 10.0 {
            *self = Fireworks::new();
            self.t0 = t;
        }
        let t = t - self.t0;
        let dt = ms as f32 / 1000.0;
        while let Some(&(when, kind, color)) = SHOW.get(self.launched)
            && t >= when
        {
            self.launched += 1;
            self.launch(t, kind, color, rng);
        }
        while self.finale_launched < FINALE.len() && t >= self.finale_due {
            let (kind, color) = FINALE[self.finale_launched];
            self.finale_launched += 1;
            self.finale_due += 0.22 + 0.2 * rng.f();
            self.launch(t, kind, color, rng);
        }
        // The shells climb, their trails behind them; at their time they burst.
        let mut bursting = Vec::new();
        for shell in &mut self.shells {
            shell.vel.z -= G * dt;
            shell.vel *= 1.0 - 0.12 * dt;
            shell.pos += shell.vel * dt;
            if t >= shell.burst_at {
                bursting.push((shell.pos, shell.kind, shell.color));
                self.nodes[shell.trail].emitter.break_path();
            } else {
                let back = -shell.vel.normalize_or(Vec3::Z);
                self.nodes[shell.trail].path(shell.pos, back, ms, rng);
            }
        }
        self.shells.retain(|s| t < s.burst_at);
        for (at, kind, color) in bursting {
            self.burst(t, at, kind, color, rng);
        }
        // What is due now.
        let mut due = std::mem::take(&mut self.due);
        due.retain(|(when, what)| {
            if t < *when {
                return true;
            }
            match what {
                Due::Split { at, vel, color } => {
                    let here = Fireworks::star_after(*at, *vel, 0.8);
                    self.nodes[self.stars]
                        .emitter
                        .set_colors(star_keys(*color, 1.0), false);
                    for _ in 0..4 {
                        self.nodes[self.stars].burst(here, Vec3::Z, 1, rng);
                    }
                }
                Due::Glint { center, spread } => {
                    // The glitter hangs in the burst's cloud, sinking a little as it goes on.
                    let a = rng.f() * TAU;
                    let z = rng.f() * 2.0 - 1.0;
                    let r = (1.0 - z * z).sqrt() * rng.f().powf(0.4) * 9.0 * spread;
                    let sink = Vec3::Z * (rng.f() * 3.0);
                    let at = *center + Vec3::new(r * a.cos(), r * a.sin(), z * 9.0 * spread) - sink;
                    self.nodes[self.glitter].burst(at, Vec3::Z, 2, rng);
                }
            }
            false
        });
        self.due = due;
        self.bursts.retain(|b| t - b.t0 < 0.5);
    }

    fn nodes(&mut self) -> &mut [Node] {
        &mut self.nodes
    }

    fn trigger(&mut self, t: f32, rng: &mut Rng) {
        let (_, kind, color) = SHOW[(rng.f() * SHOW.len() as f32) as usize % SHOW.len()];
        self.launch(t - self.t0, kind, color, rng);
    }

    /// Each burst's flash: a light over the scene that dies in half a second.
    fn lights(&self, t: f32, _frame: u32) -> Vec<HdLight> {
        let t = t - self.t0;
        self.bursts
            .iter()
            .map(|b| {
                let a = (t - b.t0) / 0.5;
                let k = (1.0 - a).clamp(0.0, 1.0).powi(2) * 40.0;
                light(
                    b.at,
                    [
                        b.color[0] * 0.6 + 0.4,
                        b.color[1] * 0.6 + 0.4,
                        b.color[2] * 0.6 + 0.4,
                    ],
                    k,
                    70.0,
                )
            })
            .collect()
    }
}
