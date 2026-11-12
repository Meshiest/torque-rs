//! A campfire under an afternoon sky: flames licking up between a teepee of logs (small tongues of light stretched along
//! their rise, white-yellow at the base through orange to deep red, flickering on fine turbulence), embers, and a thin pale
//! smoke column that rises straight at first and leans gently with a light wind higher up, widening and thinning into
//! translucent curling sheets (wisp sprites, lit by the sun from the side and the sky).

use std::ops::Range;

use glam::Vec3;
use torque_render_scene::hd::{DynamicObject, HdLight};
use torque_render_scene::particles::{EmitterData, ParticleData, ParticleExtras, Rng};

use super::{Effect, Floor, Node, Setting, emitter, group, hash, light, one, particle, plain};
use crate::scenery;
use crate::sprites::{GLOW, SMOKES, SPARK};

pub struct Smoke {
    nodes: Vec<Node>,
    smoke: Range<usize>,
    flames: usize,
    embers: usize,
    glow: usize,
}

const FIRE: Vec3 = Vec3::new(0.0, 0.0, 0.2);

impl Smoke {
    pub fn new() -> Smoke {
        let wisp = ParticleData {
            drag: 0.6,
            gravity: -0.14,
            wind: 1.0,
            lifetime_ms: 4600,
            lifetime_variance_ms: 1400,
            inv_alpha: true,
            spin_min: -20.0,
            spin_max: 20.0,
            // A thin column that widens and thins fast: pale, cool gray, mostly see-through.
            colors: [
                [0.7, 0.66, 0.62, 0.0],
                [0.78, 0.8, 0.86, 0.24],
                [0.82, 0.85, 0.92, 0.11],
                [0.85, 0.88, 0.95, 0.0],
            ],
            sizes: [0.25, 0.8, 2.1, 3.3],
            times: [0.0, 0.08, 0.5, 1.0],
            ..particle(SMOKES[0])
        };
        // Half the sprites curl on fine turbulence, half on coarse: fibers inside a slow billow.
        let wisps: Vec<(ParticleData, ParticleExtras)> = SMOKES
            .iter()
            .enumerate()
            .map(|(k, name)| {
                let (turbulence, turbulence_scale) =
                    if k % 2 == 0 { (1.1, 0.9) } else { (0.6, 2.5) };
                (
                    ParticleData {
                        textures: vec![name.to_string()],
                        ..wisp.clone()
                    },
                    ParticleExtras {
                        turbulence,
                        turbulence_scale,
                        ..ParticleExtras::default()
                    },
                )
            })
            .collect();
        // A tongue of flame: a small glow stretched along its rise, white-yellow then orange then deep red, out at the tip.
        let flame = ParticleData {
            drag: 0.9,
            gravity: -0.35,
            lifetime_ms: 420,
            lifetime_variance_ms: 180,
            colors: [
                [1.0, 0.95, 0.75, 0.0],
                [1.0, 0.8, 0.35, 0.6],
                [1.0, 0.4, 0.08, 0.35],
                [0.5, 0.08, 0.0, 0.0],
            ],
            sizes: [0.12, 0.2, 0.15, 0.05],
            times: [0.0, 0.2, 0.6, 1.0],
            ..particle(GLOW)
        };
        // An ember: born among the flames, carried up the updraft, cooling as it slows and drifts with the air above.
        let ember = ParticleData {
            drag: 0.9,
            gravity: -0.22,
            wind: 0.8,
            lifetime_ms: 2400,
            lifetime_variance_ms: 900,
            colors: [
                [1.0, 0.75, 0.3, 1.0],
                [1.0, 0.5, 0.12, 0.9],
                [0.9, 0.25, 0.03, 0.5],
                [0.4, 0.05, 0.0, 0.0],
            ],
            sizes: [0.05, 0.045, 0.035, 0.02],
            ..particle(SPARK)
        };
        // The fire's heart: a low yellow glow among the logs.
        let glow = ParticleData {
            lifetime_ms: 300,
            colors: [
                [1.0, 0.8, 0.4, 0.0],
                [1.0, 0.7, 0.3, 0.3],
                [1.0, 0.5, 0.15, 0.2],
                [1.0, 0.3, 0.0, 0.0],
            ],
            sizes: [0.3, 0.45, 0.5, 0.5],
            spin_min: -90.0,
            spin_max: 90.0,
            ..particle(GLOW)
        };
        let mut nodes = Vec::new();
        let smoke = group(
            &mut nodes,
            2,
            &EmitterData {
                period_ms: 6,
                velocity: 1.7,
                velocity_variance: 0.4,
                offset: 0.15,
                theta_max: 7.0,
                ..emitter()
            },
            &wisps,
        );
        let flames = one(
            &mut nodes,
            EmitterData {
                velocity: 1.3,
                velocity_variance: 0.6,
                theta_max: 14.0,
                ..emitter()
            },
            vec![(
                flame,
                ParticleExtras {
                    size_by_velocity: 0.1,
                    turbulence: 2.2,
                    turbulence_scale: 0.25,
                    ..ParticleExtras::default()
                },
            )],
        );
        let embers = one(
            &mut nodes,
            EmitterData {
                velocity: 1.1,
                velocity_variance: 0.5,
                theta_max: 9.0,
                ..emitter()
            },
            vec![(
                ember,
                ParticleExtras {
                    turbulence: 0.7,
                    turbulence_scale: 0.35,
                    size_by_velocity: 0.02,
                    ..ParticleExtras::default()
                },
            )],
        );
        let glow = one(
            &mut nodes,
            EmitterData {
                period_ms: 25,
                velocity: 0.1,
                velocity_variance: 0.05,
                ..emitter()
            },
            vec![plain(glow)],
        );
        Smoke {
            nodes,
            smoke,
            flames,
            embers,
            glow,
        }
    }
}

impl Smoke {
    /// A teepee of five logs leaning together over the fire.
    fn logs() -> DynamicObject {
        let mut out = Vec::new();
        for k in 0..5 {
            let a = std::f32::consts::TAU * k as f32 / 5.0 + 0.3;
            let foot = Vec3::new(a.cos() * 0.5, a.sin() * 0.5, 0.06);
            let top = Vec3::new(a.cos() * -0.08, a.sin() * -0.08, 0.72);
            let shade = (hash(k + 40) * 14.0) as u8;
            scenery::log(foot, top, 0.055, [48 + shade, 36 + shade / 2, 26], &mut out);
        }
        scenery::mesh(50, "logs", out, false, None)
    }
}

impl Effect for Smoke {
    fn setting(&self) -> Setting {
        // The fire pit: a ring of stones about a teepee of logs.
        let mut props = Vec::new();
        for k in 0..9 {
            let a = std::f32::consts::TAU * k as f32 / 9.0;
            let r = 0.78 + 0.05 * hash(k);
            let s = 0.11 + 0.05 * hash(k + 9);
            props.push((
                [a.cos() * r, a.sin() * r, s * 0.8],
                [s, s * 0.85, s * 0.8],
                [80 + (hash(k + 20) * 20.0) as u8, 76, 70],
            ));
        }
        Setting {
            // An afternoon sun off to the right, a little toward the camera: the sheets have a lit face and a sky-lit one.
            sun: ([0.85, -0.3, 0.45], [1.6, 1.35, 1.05], [0.26, 0.3, 0.38]),
            clear: [0.55, 0.62, 0.74],
            props,
            scenery: vec![Smoke::logs()],
            floor: Floor::Grass,
            atmosphere: true,
            // TGE's step takes the wind away times `windCoefficient`: this light air leans the smoke toward +x, a little away.
            wind: [-0.3, -0.08, 0.0],
            ..Setting::new([-7.0, -19.0, 3.3], [1.5, 0.0, 4.9], 50)
        }
    }

    fn step(&mut self, _t: f32, ms: u32, rng: &mut Rng) {
        // A point in the fire: anywhere over its bed, from the logs up to the flame tips.
        let in_fire = |rng: &mut Rng, radius: f32, low: f32, high: f32| {
            let a = rng.f() * std::f32::consts::TAU;
            let r = radius * rng.f().sqrt();
            FIRE + Vec3::new(a.cos() * r, a.sin() * r, low + (high - low) * rng.f())
        };
        // The smoke starts low across the whole fire, thin among the flames, and gathers into the column above.
        for k in 0..ms.div_ceil(6) {
            let i = self.smoke.start + (k as usize) % self.smoke.len();
            let at = in_fire(rng, 0.35, 0.05, 0.7);
            self.nodes[i].burst(at, Vec3::Z, 1, rng);
        }
        // Flames from anywhere among the logs, a few every step.
        for _ in 0..ms.div_ceil(4) {
            let at = in_fire(rng, 0.28, -0.05, 0.1);
            self.nodes[self.flames].burst(at, Vec3::Z, 1, rng);
        }
        // Embers up the updraft from the flames, one now and then popping out to the side.
        for _ in 0..ms.div_ceil(30) {
            let at = in_fire(rng, 0.22, 0.1, 0.5);
            let pop = rng.f() < 0.08;
            let dir = if pop {
                let a = rng.f() * std::f32::consts::TAU;
                Vec3::new(a.cos() * 0.8, a.sin() * 0.8, 1.0).normalize()
            } else {
                Vec3::Z
            };
            self.nodes[self.embers].burst(at, dir, 1, rng);
        }
        self.nodes[self.glow].path(FIRE + Vec3::Z * 0.2, Vec3::Z, ms, rng);
    }

    fn nodes(&mut self) -> &mut [Node] {
        &mut self.nodes
    }

    fn lights(&self, _t: f32, frame: u32) -> Vec<HdLight> {
        // The fire's light flickers on the logs and the ground: two wobbles, one quick and one slower.
        let flicker = 0.75 + 0.3 * hash(frame) + 0.2 * hash(frame / 5 + 77);
        vec![light(
            FIRE + Vec3::new(0.0, -0.2, 0.45),
            [2.8, 1.35, 0.45],
            flicker,
            6.0,
        )]
    }
}
