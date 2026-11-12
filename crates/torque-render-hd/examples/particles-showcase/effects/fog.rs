//! Volumetric fog: a dim barn, boarded all around, its air full of haze, and the afternoon sun coming in hard through a window
//! and a crack between boards in the wall to the camera's right. The shafts are the fog itself: the renderer's Ultra
//! volumetrics (the march and the froxel grid) light the haze where the sun and the lamps reach it and leave it dark in their
//! shadow, so the window and the crack each cast a shaft, and a lantern swinging from the loft beam sweeps its cone through the
//! haze and over the hay. A few fine motes drift in the air and shine where they cross the light. Nothing here is a sprite of
//! a shaft.

use std::ops::Range;

use glam::Vec3;
use torque_render_scene::hd::sky::Volumetric;
use torque_render_scene::hd::{DynamicObject, HdLight, HdMode, HdSpot, ImageSettings, Quality};
use torque_render_scene::particles::{EmitterData, ParticleData, Rng};

use super::{Effect, Floor, Lcg, Node, Setting, emitter, light, one, particle, plain};
use crate::scenery::{self, cuboid};
use crate::sprites::DROP;

/// The barn's inside: x and y each way from its center, its height.
const HALF: Vec3 = Vec3::new(6.0, 8.0, 0.0);
const HEIGHT: f32 = 6.0;
const BOARD: f32 = 0.15;
/// Toward the sun (unit): it comes in over the +x wall, a little from ahead of the camera.
const TOWARDS: Vec3 = Vec3::new(0.722, 0.2, 0.662);
/// The lantern: where its cord hangs from the loft beam, the cord's length, how far it swings (radians) and its period
/// (seconds).
const PIVOT: Vec3 = Vec3::new(0.6, 5.5, 4.65);
const CORD: f32 = 1.3;
const SWING: f32 = 0.62;
const PERIOD: f32 = 4.6;
/// How much the air scatters (the image setting; the engine's default is clear air).
const HAZE: f32 = 0.016;
/// The lantern's ghost id: its body is left out of its own lights' shadow casters (the lights sit inside it).
const LANTERN: u32 = 102;

/// An opening in the +x wall: y and z ranges.
struct Opening {
    y: Range<f32>,
    z: Range<f32>,
}

/// The window and the crack between two boards.
const OPENINGS: [Opening; 2] = [
    Opening {
        y: 2.0..3.6,
        z: 3.0..4.6,
    },
    Opening {
        y: -0.2..-0.05,
        z: 1.6..5.0,
    },
];

impl Opening {
    fn center(&self) -> Vec3 {
        Vec3::new(
            HALF.x,
            (self.y.start + self.y.end) * 0.5,
            (self.z.start + self.z.end) * 0.5,
        )
    }

    /// A point `along` units down the shaft from the opening, `u`, `v` (-1..1) across it.
    fn shaft(&self, along: f32, u: f32, v: f32) -> Vec3 {
        let hy = (self.y.end - self.y.start) * 0.5;
        let hz = (self.z.end - self.z.start) * 0.5;
        Vec3::new(0.0, u * hy, v * hz) + self.center() - TOWARDS * along
    }

    /// The shaft's length to the floor.
    fn length(&self) -> f32 {
        self.center().z / TOWARDS.z
    }
}

pub struct Fog {
    nodes: Vec<Node>,
    motes: usize,
    owed: f32,
}

impl Fog {
    pub fn new() -> Fog {
        // A mote: a tiny crisp point, sinking slowly on its own small drift and lit as a surface, brightening and dimming over
        // its life.
        let mote = ParticleData {
            drag: 0.8,
            gravity: 0.0018,
            lifetime_ms: 6000,
            lifetime_variance_ms: 3000,
            inv_alpha: true,
            colors: [
                [1.0, 0.98, 0.94, 0.0],
                [1.0, 0.98, 0.94, 1.0],
                [1.0, 0.98, 0.94, 0.45],
                [1.0, 0.98, 0.94, 0.0],
            ],
            sizes: [0.008, 0.012, 0.01, 0.007],
            times: [0.0, 0.3, 0.65, 1.0],
            ..particle(DROP)
        };
        let mut nodes = Vec::new();
        let motes = one(
            &mut nodes,
            EmitterData {
                velocity: 0.025,
                velocity_variance: 0.02,
                theta_max: 180.0,
                ..emitter()
            },
            vec![plain(mote)],
        );
        Fog {
            nodes,
            motes,
            owed: 0.0,
        }
    }

    /// The lantern at `t`: where it hangs and the way it points (down its cord, tilted a little toward the camera).
    fn lantern(t: f32) -> (Vec3, Vec3) {
        let a = SWING * (std::f32::consts::TAU * t / PERIOD).sin();
        let down = Vec3::new(a.sin(), 0.0, -a.cos());
        let at = PIVOT + down * CORD;
        let dir = (down + Vec3::new(0.0, -0.35, 0.0)).normalize();
        (at, dir)
    }

    /// The barn: board walls with the openings in the +x wall, a roof, a loft beam, and what stands in the light: hay bales
    /// and a barrel.
    fn scenery() -> Vec<DynamicObject> {
        let wood = [92, 76, 60];
        let dark = [64, 52, 42];
        let mut walls = Vec::new();
        let z_mid = HEIGHT * 0.5;
        // The end walls, the wall on the camera's left, the roof.
        for sign in [-1.0, 1.0] {
            cuboid(
                Vec3::new(0.0, sign * (HALF.y + BOARD), z_mid),
                Vec3::new(HALF.x + 2.0 * BOARD, BOARD, z_mid),
                wood,
                &mut walls,
            );
        }
        cuboid(
            Vec3::new(-(HALF.x + BOARD), 0.0, z_mid),
            Vec3::new(BOARD, HALF.y, z_mid),
            wood,
            &mut walls,
        );
        cuboid(
            Vec3::new(0.0, 0.0, HEIGHT + BOARD),
            Vec3::new(HALF.x + 2.0 * BOARD, HALF.y + 2.0 * BOARD, BOARD),
            dark,
            &mut walls,
        );
        // The sunny wall: boards from the floor to the roof with the openings cut out.
        let mut cuts: Vec<(f32, f32)> = OPENINGS.iter().map(|o| (o.y.start, o.y.end)).collect();
        cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
        let x = HALF.x + BOARD;
        let mut y0 = -HALF.y;
        for (a, b) in &cuts {
            cuboid(
                Vec3::new(x, (y0 + a) * 0.5, z_mid),
                Vec3::new(BOARD, (a - y0) * 0.5, z_mid),
                wood,
                &mut walls,
            );
            y0 = *b;
        }
        cuboid(
            Vec3::new(x, (y0 + HALF.y) * 0.5, z_mid),
            Vec3::new(BOARD, (HALF.y - y0) * 0.5, z_mid),
            wood,
            &mut walls,
        );
        // Above and below each opening.
        for o in &OPENINGS {
            let y = (o.y.start + o.y.end) * 0.5;
            let hy = (o.y.end - o.y.start) * 0.5;
            cuboid(
                Vec3::new(x, y, o.z.start * 0.5),
                Vec3::new(BOARD, hy, o.z.start * 0.5),
                wood,
                &mut walls,
            );
            let top = (o.z.end + HEIGHT) * 0.5;
            cuboid(
                Vec3::new(x, y, top),
                Vec3::new(BOARD, hy, HEIGHT - top),
                wood,
                &mut walls,
            );
        }
        // A loft beam across the barn and the posts under it, behind the shafts.
        cuboid(
            Vec3::new(0.0, 5.5, 4.8),
            Vec3::new(HALF.x, 0.14, 0.14),
            dark,
            &mut walls,
        );
        for px in [-4.2, 4.2] {
            cuboid(
                Vec3::new(px, 5.5, 2.4),
                Vec3::new(0.14, 0.14, 2.4),
                dark,
                &mut walls,
            );
        }
        // Hay bales where the window's shaft lands, a barrel by the crack's, a stack of bales under the lantern.
        let mut hay = Vec::new();
        let mut g = Lcg(31);
        let land = OPENINGS[0].shaft(OPENINGS[0].length(), 0.0, 0.0);
        for (k, (dx, dy, dz)) in [(0.1, 0.5, 0.0), (0.3, -0.5, 0.0), (0.2, 0.0, 0.5)]
            .into_iter()
            .enumerate()
        {
            let shade = (g.unit() * 20.0) as u8;
            cuboid(
                land + Vec3::new(dx, dy, dz + 0.25),
                Vec3::new(0.3, 0.45, 0.25),
                [150 + shade, 125 + shade, 60 + k as u8 * 4],
                &mut hay,
            );
        }
        for (k, (dx, dy, dz)) in [(-0.4, 0.0, 0.0), (0.5, 0.1, 0.0), (0.0, 0.05, 0.5)]
            .into_iter()
            .enumerate()
        {
            let shade = (g.unit() * 20.0) as u8;
            cuboid(
                Vec3::new(PIVOT.x + dx, PIVOT.y - 1.0 + dy, dz + 0.25),
                Vec3::new(0.45, 0.32, 0.25),
                [145 + shade, 120 + shade, 58 + k as u8 * 4],
                &mut hay,
            );
        }
        let barrel = OPENINGS[1].shaft(OPENINGS[1].length() - 0.3, 0.0, 0.0);
        cuboid(
            Vec3::new(barrel.x, barrel.y - 0.6, 0.45),
            Vec3::new(0.32, 0.32, 0.45),
            [74, 60, 48],
            &mut hay,
        );
        vec![
            scenery::mesh(100, "barn", walls, false, None),
            scenery::mesh(101, "hay", hay, false, None),
        ]
    }
}

impl Effect for Fog {
    fn setting(&self) -> Setting {
        Setting {
            // A hard afternoon sun; the room gets only what little the openings let in.
            sun: (TOWARDS.to_array(), [2.8, 2.45, 1.9], [0.09, 0.092, 0.105]),
            clear: [0.72, 0.8, 0.95],
            scenery: Fog::scenery(),
            floor: Floor::Planks,
            // The haze: thin fog from the eye out as the volumetric light's medium, dark where no light reaches it.
            fog: Some(([0.085, 0.085, 0.095], 0.0, 140.0)),
            ..Setting::new([-2.5, -7.0, 1.6], [1.0, 3.0, 1.8], 58)
        }
    }

    fn mode(&self) -> HdMode {
        HdMode {
            image: ImageSettings {
                volumetric: Volumetric {
                    scattering: HAZE,
                    ..ImageSettings::DEFAULT.volumetric
                },
                ..ImageSettings::DEFAULT
            },
            quality: Quality::Ultra,
        }
    }

    fn step(&mut self, _t: f32, ms: u32, rng: &mut Rng) {
        let dt = ms as f32 / 1000.0;
        // A few motes anywhere in the room's air.
        self.owed += dt * 260.0;
        while self.owed >= 1.0 {
            self.owed -= 1.0;
            let at = Vec3::new(
                (rng.f() * 2.0 - 1.0) * (HALF.x - 0.3),
                (rng.f() * 2.0 - 1.0) * (HALF.y - 0.3),
                0.2 + rng.f() * (HEIGHT - 0.6),
            );
            self.nodes[self.motes].burst(at, Vec3::Z, 1, rng);
        }
    }

    fn nodes(&mut self) -> &mut [Node] {
        &mut self.nodes
    }

    /// The lantern's own glow about it.
    fn lights(&self, t: f32, frame: u32) -> Vec<HdLight> {
        let (at, _) = Fog::lantern(t);
        let flicker = 0.9 + 0.1 * super::hash(frame / 2);
        vec![HdLight {
            carrier: Some(LANTERN),
            ..light(at, [1.0, 0.72, 0.42], 1.6 * flicker, 5.0)
        }]
    }

    /// The lantern's cone, swinging, its shadows in the haze.
    fn spots(&self, t: f32) -> Vec<HdSpot> {
        let (at, dir) = Fog::lantern(t);
        vec![HdSpot {
            light: HdLight {
                carrier: Some(LANTERN),
                ..light(at - dir * 0.1, [1.0, 0.78, 0.5], 34.0, 16.0)
            },
            direction: dir.to_array(),
            inner: 14f32.to_radians(),
            outer: 32f32.to_radians(),
            shadows: true,
        }]
    }

    /// The lantern: a small dark box on its cord.
    fn meshes(&self, t: f32) -> Vec<DynamicObject> {
        let (at, _) = Fog::lantern(t);
        let mut out = Vec::new();
        cuboid(at, Vec3::new(0.09, 0.09, 0.12), [40, 34, 30], &mut out);
        scenery::log(PIVOT, at + Vec3::Z * 0.12, 0.008, [50, 44, 38], &mut out);
        vec![scenery::mesh(LANTERN, "lantern", out, false, None)]
    }
}
