//! Petals in the wind: cherry trees in bloom at the edge of a meadow on a bright spring morning, a light air across it
//! and a gust every so often. Each gust shakes petals loose from the canopies, and they are carried across the meadow:
//! they fall slowly (a petal's terminal speed is well under a walking pace), tumble (a flipbook of the petal seen at
//! tilts, stepped as it turns) and flutter (a spin about their own axis), swirl in the eddies the turbulence makes behind
//! the trees, and settle on the grass, where they lie until they fade. Lit sprites: the sun comes through them when the
//! eye looks toward it. Space brings a gust.

use std::f32::consts::TAU;

use glam::Vec3;
use torque_render_scene::hd::{DynamicObject, HdLight};
use torque_render_scene::particles::{EmitterData, ParticleData, ParticleExtras, Rng};

use super::{Effect, Floor, Lcg, Node, Setting, emitter, one, particle, smooth};
use crate::scenery;
use crate::sprites::PETALS;

/// The still air's velocity (units a second) and a gust's at its height (the scene's wind is this taken away: see
/// [`Setting::wind`]).
const BREEZE: Vec3 = Vec3::new(0.7, 0.15, 0.0);
const GUST: Vec3 = Vec3::new(3.2, 0.6, 0.0);
/// A gust's seconds, and the seconds between gusts.
const GUST_LENGTH: f32 = 3.2;
const GUST_GAP: (f32, f32) = (4.0, 8.0);

/// A tree in bloom: its foot, its trunk's height and the crown's blossom tufts (center, radii).
struct Tree {
    foot: Vec3,
    height: f32,
    lobes: Vec<(Vec3, Vec3)>,
    seed: u64,
}

impl Tree {
    fn new(foot: Vec3, height: f32, g: &mut Lcg) -> Tree {
        let spread = g.range(2.2, 2.7);
        let crown = foot + Vec3::Z * (height + spread * 0.5);
        let lobes = scenery::crown_tufts(
            crown,
            Vec3::new(spread, spread * g.range(0.85, 1.0), spread * 0.7),
            120,
            0.13,
            g,
        );
        Tree {
            foot,
            height,
            lobes,
            seed: (g.unit() * 1000.0) as u64,
        }
    }

    /// A point on the canopy's surface (where the petals hang), on the windward side more than the lee.
    fn blossom(&self, rng: &mut Rng) -> Vec3 {
        let (center, radii) =
            self.lobes[(rng.f() * self.lobes.len() as f32) as usize % self.lobes.len()];
        let z = rng.f() * 2.0 - 1.0;
        let a = rng.f() * TAU;
        let s = (1.0 - z * z).sqrt();
        center + Vec3::new(s * a.cos(), s * a.sin(), z) * radii * 0.95
    }
}

pub struct Petals {
    nodes: Vec<Node>,
    petals: usize,
    trees: Vec<Tree>,
    gust_at: f32,
    next_gust: f32,
    owed: f32,
}

impl Petals {
    pub fn new() -> Petals {
        // A petal: pastel pink, lit, tumbling through the flipbook's tilts as it spins; it fades where it lies at the end.
        let petal = ParticleData {
            drag: 1.1,
            gravity: 0.055,
            wind: 1.0,
            lifetime_ms: 11000,
            lifetime_variance_ms: 4000,
            inv_alpha: true,
            spin_min: -140.0,
            spin_max: 140.0,
            colors: [
                [1.0, 0.82, 0.88, 1.0],
                [1.0, 0.8, 0.87, 1.0],
                [0.98, 0.78, 0.85, 1.0],
                [0.95, 0.75, 0.82, 0.0],
            ],
            sizes: [0.1, 0.1, 0.1, 0.1],
            times: [0.0, 0.5, 0.88, 1.0],
            textures: PETALS.iter().map(|s| s.to_string()).collect(),
            animate: true,
            fps: 9,
            ..particle(PETALS[0])
        };
        let mut nodes = Vec::new();
        let petals = one(
            &mut nodes,
            EmitterData {
                velocity: 0.3,
                velocity_variance: 0.2,
                theta_max: 40.0,
                ..emitter()
            },
            vec![(
                petal,
                ParticleExtras {
                    turbulence: 0.9,
                    turbulence_scale: 1.3,
                    collide: true,
                    bounce_elasticity: 0.05,
                    bounce_friction: 0.75,
                    ..ParticleExtras::default()
                },
            )],
        );
        let mut g = Lcg(83);
        let trees = vec![
            Tree::new(Vec3::new(-7.5, 4.0, 0.0), 3.0, &mut g),
            Tree::new(Vec3::new(-4.0, 9.5, 0.0), 3.6, &mut g),
            Tree::new(Vec3::new(-10.5, 11.0, 0.0), 3.3, &mut g),
        ];
        Petals {
            nodes,
            petals,
            trees,
            gust_at: -10.0,
            next_gust: 2.0,
            owed: 0.0,
        }
    }

    /// The gust's strength at `t` (0..1): it rises fast and dies away.
    fn gust(&self, t: f32) -> f32 {
        let u = (t - self.gust_at) / GUST_LENGTH;
        smooth(u * 5.0) * (1.0 - smooth(u))
    }

    /// The cherry trees, the grass and the tree line behind the meadow.
    fn scenery(&self) -> Vec<DynamicObject> {
        let mut trees = Vec::new();
        let mut g = Lcg(89);
        for tree in &self.trees {
            scenery::tree(
                tree.foot,
                tree.height,
                &tree.lobes,
                [62, 48, 40],
                [240, 190, 204],
                tree.seed,
                &mut trees,
            );
        }
        let mut grass = Vec::new();
        scenery::blades(91, 5000, -10.0..10.0, -4.0..8.0, &mut grass);
        scenery::blades(92, 2500, -14.0..14.0, 8.0..18.0, &mut grass);
        // A line of green trees far across the meadow.
        let mut woods = Vec::new();
        for k in 0..24 {
            let x = -66.0 + 5.5 * k as f32 + g.range(-1.5, 1.5);
            let y = g.range(58.0, 74.0);
            let height = g.range(2.5, 4.5);
            let foot = Vec3::new(x, y, 0.0);
            let r = g.range(2.2, 3.2);
            let tufts = scenery::crown_tufts(
                foot + Vec3::Z * (height + r * 0.6),
                Vec3::new(r, r, r * 1.1),
                14,
                0.42,
                &mut g,
            );
            scenery::tree(
                foot,
                height,
                &tufts,
                [40, 32, 26],
                [44, 76, 38],
                400 + k as u64,
                &mut woods,
            );
        }
        vec![
            scenery::mesh(100, "trees", trees, false, None),
            scenery::mesh(101, "grass", grass, false, None),
            scenery::mesh(102, "woods", woods, false, None),
        ]
    }
}

impl Effect for Petals {
    fn setting(&self) -> Setting {
        Setting {
            // A spring morning: the sun up to the right and behind the trees, so the petals are lit through.
            sun: ([0.45, 0.6, 0.66], [1.5, 1.35, 1.15], [0.3, 0.34, 0.42]),
            clear: [0.62, 0.74, 0.9],
            scenery: self.scenery(),
            floor: Floor::Grass,
            atmosphere: true,
            wind: (-BREEZE).to_array(),
            ..Setting::new([-1.0, -7.5, 1.6], [-2.5, 5.0, 2.4], 56)
        }
    }

    fn step(&mut self, t: f32, ms: u32, rng: &mut Rng) {
        let dt = ms as f32 / 1000.0;
        if t >= self.next_gust {
            self.gust_at = t;
            self.next_gust = t + GUST_LENGTH + GUST_GAP.0 + (GUST_GAP.1 - GUST_GAP.0) * rng.f();
        }
        let gust = self.gust(t);
        // Petals shaken loose: a few in the breeze, many in a gust, from the canopies, carried off in the air's direction.
        self.owed += dt * (14.0 + 160.0 * gust);
        let air = BREEZE + (GUST - BREEZE) * gust;
        while self.owed >= 1.0 {
            self.owed -= 1.0;
            let tree = &self.trees[(rng.f() * self.trees.len() as f32) as usize % self.trees.len()];
            let at = tree.blossom(rng);
            self.nodes[self.petals].burst(at, (air * 0.4 + Vec3::Z * 0.1).normalize(), 1, rng);
        }
    }

    fn nodes(&mut self) -> &mut [Node] {
        &mut self.nodes
    }

    fn lights(&self, _t: f32, _frame: u32) -> Vec<HdLight> {
        Vec::new()
    }

    fn wind(&self, t: f32) -> Option<[f32; 3]> {
        Some((-(BREEZE + (GUST - BREEZE) * self.gust(t))).to_array())
    }

    fn trigger(&mut self, t: f32, _rng: &mut Rng) {
        self.gust_at = t;
        self.next_gust = t + GUST_LENGTH + GUST_GAP.0;
    }
}
