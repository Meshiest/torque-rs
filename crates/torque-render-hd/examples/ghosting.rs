//! The ghosting check: scripted scenes drawn twice, once as a window draws them (every pass keeping its history) and once as a
//! reference with no history at all: at each instant the scene is frozen, every history started again, and the frame drawn until
//! its samples settle (the last frames averaged), so the reference is the noise-free image of that instant. The difference is
//! what the histories add: a trail, a lag, a smear.
//!
//! The scenes: a still camera with a buggy-sized box driving past over a dirt ground (its shadow and occlusion sweep the
//! ground), the sun moving fast (a day sped up), a light switched on and off, the camera moving over still ground, smoke
//! rising then gone, the sky's clouds under a fast sun and then a turning camera (the clouds' own history), a water mirror
//! (at Medium: the screen-space reflections' history) with the buggy driving along it and then the camera moving, thin
//! poles far off under a moving camera (sub-pixel features against the history's clip), and at Ultra lights in a thick fog
//! (the froxel grid): the buggy driving past with its headlights on at dusk, then a lamp switched on and off. Each starts with
//! frames that settle, then the change, then frames held still; a scene with two changes holds after each.
//!
//! Per scene: `floor` the share of pixels off the reference once settled before the change (the noise left), `worst` and `mean`
//! the share off from the change on, `settle` the frames after each change ends (the motion stopped, the light switched) until
//! the image stays within the tolerance, `trail` the worst share off after a change ended. A pixel is off where a channel is
//! more than [`TOLERANCE`] of 255 from the reference; a frame is within where at most [`AREA_OK`] of its pixels are off.
//!
//! `cargo run -p torque-render-hd --release --example ghosting -- [--audit] [--shot <name>] [--out <dir>] [--max-settle <frames>]
//! [--max-trail <percent>] [--quality low|medium|high|ultra] [--trace] [--gpu-particles] [--march]`: `--audit` draws each
//! scene again with each pass's history off on its own (`HdWorld::histories`) and the reflection capture drawn whole every
//! frame; `--out` writes the worst frame of each scene, its reference and their difference (`--frame <n>`: that frame too);
//! `--quality` draws every scene at that quality in place of its own; `--march` draws the volumetric light with the march alone
//! at every quality (the froxel grid off, `HdFeatures::froxel_fog`); `--trace` prints each frame's share off. Each row ends with
//! the run's GPU time a frame (ms, the whole frame, the sun mask's pass and the anti-aliasing's; with the device's timestamps).
//! The command fails when a gated scene drawn by default passes the limits (by default `ghosting::MAX_SETTLE` frames to settle
//! and a `ghosting::MAX_TRAIL` % trail); the cloud, mirror and thin-far scenes report their numbers without gating (their
//! histories are measured here before they are worked on). `--gpu-particles` draws the smoke through the GPU particle pool
//! (`HdFeatures::gpu_particles`: each frame's puffs handed over afresh, camera-facing) instead of the CPU path's quads.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use torque_render_core::Gpu;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_hd::ghosting::{self, AREA_OK, Image, MAX_SETTLE, MAX_TRAIL, Measure, TOLERANCE};
use torque_render_hd::{HdWorld, ParticlePool};
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::kit::{Lcg, box_object, look_at};
use torque_render_scene::hd::scatter::{ScatterLook, ScatterTable};
use torque_render_scene::hd::{
    DynamicObject, HdCamera, HdDraw, HdLight, HdMode, HdScene, HdSpot, Quality, material,
};
use torque_render_scene::particles::{GpuSpawn, SpriteFrame, SpriteLook};
use torque_render_scene::{Blend, Cull, Draw, Filter, Sampler, TexEnv, Texture, Vertex};

const SIZE: (u32, u32) = (384, 216);
const FOV: u8 = 70;
/// The frames each scene settles before its change, and holds after it.
const SETTLE: usize = 48;
const HOLD: usize = 48;
const BUGGY: u32 = 1;

/// What a scene shows at one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
struct State {
    eye: [f32; 3],
    target: [f32; 3],
    /// The sun's azimuth (degrees from +y toward +x) and elevation (degrees; under 0 dusk).
    sun: [f32; 2],
    /// The buggy's place (its middle on the ground).
    buggy: Option<[f32; 3]>,
    light: bool,
    /// The buggy's two headlights.
    headlights: bool,
    /// The smoke's age (frames), none: no smoke.
    smoke: Option<f32>,
    /// The frame itself, where the scene moves with the time though nothing else changes (the water's waves): no two frames share
    /// a reference.
    frame: Option<usize>,
}

const BASE: State = State {
    eye: [-1.0, -8.0, 4.0],
    target: [0.0, 1.5, 0.2],
    sun: [-40.0, 32.0],
    buggy: None,
    light: false,
    headlights: false,
    smoke: None,
    frame: None,
};

/// A scripted scene: its name, its quality, its fixtures, its frames after [`SETTLE`] (the changes and the holds), the frames
/// (from 0) each change starts and ends at, and its state at each frame.
struct Shot {
    name: &'static str,
    quality: Quality,
    /// A water plane over the ground (the mirror), thin poles far off, a thick fog from a few units off.
    water: bool,
    poles: bool,
    fog: bool,
    /// Whether the scene's numbers fail the command past the limits.
    gated: bool,
    frames: usize,
    changes: Vec<(usize, usize)>,
    state: Box<dyn Fn(usize) -> State>,
}

/// A scene at High on the plain ground.
fn plain(
    name: &'static str,
    frames: usize,
    changes: Vec<(usize, usize)>,
    state: impl Fn(usize) -> State + 'static,
) -> Shot {
    Shot {
        name,
        quality: Quality::High,
        water: false,
        poles: false,
        fog: false,
        gated: true,
        frames,
        changes,
        state: Box::new(state),
    }
}

/// A scene whose numbers are reported, not gated.
fn reported(shot: Shot) -> Shot {
    Shot {
        gated: false,
        ..shot
    }
}

/// The frames' index after the settling ones (negative: settling).
fn after(f: usize) -> f32 {
    f as f32 - SETTLE as f32
}

fn shots() -> Vec<Shot> {
    let drive = 70;
    let sun = 60;
    let walk = 60;
    let smoke = 60;
    vec![
        plain(
            "buggy-shadow",
            SETTLE + drive + HOLD,
            vec![(SETTLE, SETTLE + drive)],
            move |f| {
                let t = after(f).clamp(0.0, drive as f32);
                State {
                    buggy: Some([-9.0 + 0.25 * t, 1.0, 0.0]),
                    ..BASE
                }
            },
        ),
        plain(
            "sun-fast",
            SETTLE + sun + HOLD,
            vec![(SETTLE, SETTLE + sun)],
            move |f| {
                let t = after(f).clamp(0.0, sun as f32);
                State {
                    sun: [-40.0 + 1.0 * t, 32.0 - 0.2 * t],
                    buggy: Some([0.0, 1.0, 0.0]),
                    ..BASE
                }
            },
        ),
        plain(
            "light-toggle",
            SETTLE + 30 + HOLD,
            vec![(SETTLE, SETTLE), (SETTLE + 30, SETTLE + 30)],
            |f| State {
                sun: [-40.0, -3.0],
                buggy: Some([0.0, 1.0, 0.0]),
                light: (SETTLE..SETTLE + 30).contains(&f),
                ..BASE
            },
        ),
        plain(
            "camera-move",
            SETTLE + walk + HOLD,
            vec![(SETTLE, SETTLE + walk)],
            move |f| {
                let t = after(f).clamp(0.0, walk as f32);
                State {
                    eye: [-1.0 + 0.08 * t, -8.0 + 0.03 * t, 4.0],
                    target: [0.0 + 0.05 * t, 1.5, 0.2],
                    buggy: Some([0.0, 1.0, 0.0]),
                    ..BASE
                }
            },
        ),
        plain(
            "smoke",
            SETTLE + smoke + HOLD,
            vec![(SETTLE, SETTLE + smoke)],
            move |f| {
                let t = after(f);
                State {
                    buggy: Some([0.0, 1.0, 0.0]),
                    smoke: (0.0..smoke as f32).contains(&t).then_some(t),
                    ..BASE
                }
            },
        ),
        // The sky's clouds: the sun moving fast (their light changes), held, then the camera turning (their history reprojected).
        reported(plain(
            "clouds",
            SETTLE + sun + HOLD + walk + HOLD,
            vec![
                (SETTLE, SETTLE + sun),
                (SETTLE + sun + HOLD, SETTLE + sun + HOLD + walk),
            ],
            move |f| {
                let t = after(f).clamp(0.0, sun as f32);
                let u = (after(f) - (sun + HOLD) as f32).clamp(0.0, walk as f32);
                State {
                    target: [0.1 * u, 10.0, 14.0],
                    sun: [-40.0 + 1.0 * t, 32.0 - 0.2 * t],
                    ..BASE
                }
            },
        )),
        // A water mirror at Medium (its reflection is the screen-space reflections', with their history): the buggy driving along
        // it, held, then the camera moving.
        Shot {
            name: "mirror",
            quality: Quality::Medium,
            water: true,
            poles: false,
            fog: false,
            gated: false,
            frames: SETTLE + drive + HOLD + walk + HOLD,
            changes: vec![
                (SETTLE, SETTLE + drive),
                (SETTLE + drive + HOLD, SETTLE + drive + HOLD + walk),
            ],
            state: Box::new(move |f| {
                let t = after(f).clamp(0.0, drive as f32);
                let u = (after(f) - (drive + HOLD) as f32).clamp(0.0, walk as f32);
                State {
                    eye: [-1.0 + 0.08 * u, -8.0 + 0.03 * u, 1.6],
                    target: [0.05 * u, 1.5, 0.6],
                    buggy: Some([-9.0 + 0.25 * t, 3.0, 0.0]),
                    frame: Some(f),
                    ..BASE
                }
            }),
        },
        // Thin poles far off (sub-pixel features): the camera moving slowly, then held.
        Shot {
            name: "thin-far",
            quality: Quality::High,
            water: false,
            poles: true,
            fog: false,
            gated: false,
            frames: SETTLE + walk + HOLD,
            changes: vec![(SETTLE, SETTLE + walk)],
            state: Box::new(move |f| {
                let t = after(f).clamp(0.0, walk as f32);
                State {
                    eye: [-1.0 + 0.03 * t, -8.0, 4.0],
                    target: [0.02 * t, 40.0, 3.0],
                    ..BASE
                }
            }),
        },
        // Lights in a thick fog at Ultra (the froxel grid): at dusk the buggy driving past with its headlights on, held,
        // then a lamp switched on and off.
        Shot {
            name: "fog-lights",
            quality: Quality::Ultra,
            water: false,
            poles: false,
            fog: true,
            gated: true,
            frames: SETTLE + drive + HOLD + 30 + HOLD,
            changes: vec![
                (SETTLE, SETTLE + drive),
                (SETTLE + drive + HOLD, SETTLE + drive + HOLD),
                (SETTLE + drive + HOLD + 30, SETTLE + drive + HOLD + 30),
            ],
            state: Box::new(move |f| {
                let t = after(f).clamp(0.0, drive as f32);
                let on = SETTLE + drive + HOLD;
                State {
                    sun: [-40.0, -3.0],
                    buggy: Some([-9.0 + 0.25 * t, 1.0, 0.0]),
                    headlights: true,
                    light: (on..on + 30).contains(&f),
                    ..BASE
                }
            }),
        },
    ]
}

/// A dirt ground's texture: brown value noise over three octaves with a fine grain (the clip box of a busy ground is wide).
fn dirt() -> Texture {
    const N: usize = 256;
    let mut rng = Lcg::new(7);
    let lattice = |cells: usize, rng: &mut Lcg| -> Vec<f32> {
        (0..cells * cells).map(|_| rng.unit()).collect()
    };
    let octaves = [(8usize, 0.5f32), (32, 0.3), (128, 0.2)];
    let grids: Vec<(usize, f32, Vec<f32>)> = octaves
        .iter()
        .map(|&(c, a)| (c, a, lattice(c, &mut rng)))
        .collect();
    let mut rgba = Vec::with_capacity(N * N * 4);
    for y in 0..N {
        for x in 0..N {
            let mut v = 0.0;
            for (c, a, g) in &grids {
                let (fx, fy) = (
                    x as f32 * *c as f32 / N as f32,
                    y as f32 * *c as f32 / N as f32,
                );
                let (ix, iy) = (fx.floor() as usize, fy.floor() as usize);
                let (tx, ty) = (fx - ix as f32, fy - iy as f32);
                let at = |i: usize, j: usize| g[(j % c) * c + (i % c)];
                let top = at(ix, iy) * (1.0 - tx) + at(ix + 1, iy) * tx;
                let bottom = at(ix, iy + 1) * (1.0 - tx) + at(ix + 1, iy + 1) * tx;
                v += a * (top * (1.0 - ty) + bottom * ty);
            }
            let grain = rng.unit() * 0.12;
            let l = (0.55 + 0.6 * (v - 0.5) + grain - 0.06).clamp(0.0, 1.0);
            let c = [l * 170.0, l * 132.0, l * 96.0].map(|c| c as u8);
            rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    Texture {
        format: torque_render_scene::TexelFormat::Rgba8,
        path: "ghosting dirt".into(),
        width: N as u32,
        height: N as u32,
        levels: Arc::new(vec![rgba]),
    }
}

/// A soft round puff (the smoke's sprite).
fn puff() -> Texture {
    const N: usize = 32;
    let mut rgba = Vec::with_capacity(N * N * 4);
    for y in 0..N {
        for x in 0..N {
            let d = ((x as f32 + 0.5) / N as f32 * 2.0 - 1.0)
                .hypot((y as f32 + 0.5) / N as f32 * 2.0 - 1.0);
            let a = (1.0 - d).clamp(0.0, 1.0).powf(1.5);
            rgba.extend_from_slice(&[255, 255, 255, (a * 255.0) as u8]);
        }
    }
    Texture {
        format: torque_render_scene::TexelFormat::Rgba8,
        path: "ghosting puff".into(),
        width: N as u32,
        height: N as u32,
        levels: Arc::new(vec![rgba]),
    }
}

fn sampler() -> Sampler {
    Sampler {
        repeat: true,
        mag: Filter::Linear,
        min: Filter::Linear,
        mip: Filter::Linear,
        mipmaps: true,
    }
}

/// The ground: `half` units each way at height 0, the dirt every 4 units.
fn ground(half: f32) -> Draw {
    let corner = |x: f32, y: f32| Vertex {
        position: [x, y, 0.0],
        normal: [0.0, 0.0, 1.0],
        color: [255; 4],
        uv: [x / 4.0, y / 4.0],
    };
    let (a, b, c, d) = (
        corner(-half, -half),
        corner(half, -half),
        corner(half, half),
        corner(-half, half),
    );
    Draw {
        label: "ground".into(),
        vertices: vec![a, b, c, a, c, d],
        lighting: true,
        tex_env: TexEnv::Modulate,
        texture: Some((0, sampler())),
        blend: Blend::Opaque,
        cull: Cull::None,
        depth_test: true,
        depth_write: true,
        fog: true,
        lights: Vec::new(),
        vertex_key: None,
    }
}

fn tinted(id: u32, center: [f32; 3], half: [f32; 3], color: [u8; 3]) -> DynamicObject {
    let mut o = box_object(id, center, half);
    for d in &mut o.draws {
        for v in &mut d.vertices {
            v.color = [color[0], color[1], color[2], 255];
        }
    }
    o
}

/// The buggy at `at` (a body over four wheels, one object), moved from `before`.
fn buggy(at: [f32; 3], before: [f32; 3]) -> DynamicObject {
    let [x, y, _] = at;
    let mut car = tinted(BUGGY, [x, y, 0.75], [1.5, 0.85, 0.3], [170, 40, 35]);
    let parts = [
        tinted(BUGGY, [x - 0.3, y, 1.3], [0.7, 0.7, 0.25], [60, 70, 90]),
        tinted(
            BUGGY,
            [x - 1.0, y - 0.95, 0.4],
            [0.4, 0.2, 0.4],
            [30, 30, 30],
        ),
        tinted(
            BUGGY,
            [x + 1.0, y - 0.95, 0.4],
            [0.4, 0.2, 0.4],
            [30, 30, 30],
        ),
        tinted(
            BUGGY,
            [x - 1.0, y + 0.95, 0.4],
            [0.4, 0.2, 0.4],
            [30, 30, 30],
        ),
        tinted(
            BUGGY,
            [x + 1.0, y + 0.95, 0.4],
            [0.4, 0.2, 0.4],
            [30, 30, 30],
        ),
    ];
    for p in parts {
        let base = car.draws[0].vertices.len() as u32;
        let d = &mut car.draws[0];
        d.vertices.extend_from_slice(&p.draws[0].vertices);
        d.indices
            .extend(p.draws[0].indices.iter().map(|i| i + base));
    }
    car.centre = [x, y, 0.8];
    car.radius = 2.1;
    car.transform.set_column3(at);
    car.prev_transform.set_column3(before);
    car
}

/// One of the buggy's headlights (the buggy at `at`, `dy` across its front), pointing along its way and a little down.
fn headlight(at: [f32; 3], dy: f32) -> HdSpot {
    let range = 30.0f32;
    let gain = 0.5 * (1.0 + 0.25 * (range / 2.0).powi(2));
    HdSpot {
        light: HdLight {
            position: [at[0] + 1.6, at[1] + dy, 0.8],
            color: [1.0, 0.88, 0.7].map(|c: f32| c * gain),
            radius: range,
            carrier: Some(BUGGY),
        },
        direction: [1.0, 0.0, -0.1],
        inner: 9f32.to_radians(),
        outer: 22f32.to_radians(),
        shadows: true,
    }
}

/// The still blocks about the track.
fn blocks() -> Vec<DynamicObject> {
    vec![
        tinted(2, [-4.0, 5.0, 1.0], [1.0, 1.0, 1.0], [210, 200, 180]),
        tinted(3, [3.5, 7.0, 1.5], [0.6, 0.6, 1.5], [90, 140, 200]),
        tinted(4, [5.5, -2.0, 0.4], [1.5, 0.4, 0.4], [120, 180, 90]),
    ]
}

/// Thin poles far off, a pixel wide or less at this size: a row across the view at 60 to 110 units and a thin bar among them.
fn poles() -> Vec<DynamicObject> {
    let mut rng = Lcg::new(5);
    let mut out = Vec::with_capacity(41);
    for k in 0..40 {
        let x = -30.0 + 1.5 * k as f32 + rng.unit();
        let y = 60.0 + 50.0 * rng.unit();
        let h = 1.5 + 2.5 * rng.unit();
        out.push(tinted(10 + k, [x, y, h], [0.05, 0.05, h], [70, 60, 50]));
    }
    out.push(tinted(
        50,
        [0.0, 85.0, 3.0],
        [30.0, 0.04, 0.04],
        [90, 80, 70],
    ));
    out
}

/// A water plane just over the ground (the mirror).
fn water(half: f32) -> Draw {
    let corner = |x: f32, y: f32| Vertex {
        position: [x, y, 0.02],
        normal: [0.0, 0.0, 1.0],
        color: [40, 80, 90, 128],
        uv: [0.0; 2],
    };
    let (a, b, c, d) = (
        corner(-half, -half),
        corner(half, -half),
        corner(half, half),
        corner(-half, half),
    );
    Draw {
        label: "water".into(),
        vertices: vec![a, b, c, a, c, d],
        lighting: true,
        tex_env: TexEnv::None,
        texture: None,
        blend: Blend::Alpha,
        cull: Cull::None,
        depth_test: true,
        depth_write: false,
        fog: true,
        lights: Vec::new(),
        vertex_key: None,
    }
}

/// The smoke's puffs at `age` frames: each one's center, half size and alpha.
fn puffs(age: f32) -> Vec<([f32; 3], f32, f32)> {
    let mut rng = Lcg::new(11);
    let mut out = Vec::new();
    for k in 0..24 {
        let born = k as f32 * 2.5;
        let (dx, dy) = (rng.unit() - 0.5, rng.unit() - 0.5);
        let life = age - born;
        if life < 0.0 {
            continue;
        }
        let c = [
            -1.8 - 0.03 * life + dx,
            1.0 + dy + 0.01 * life,
            0.4 + 0.035 * life,
        ];
        let half = 0.35 + 0.012 * life;
        let alpha = (0.7 * (1.0 - life / 60.0)).clamp(0.0, 1.0);
        out.push((c, half, alpha));
    }
    out
}

/// The smoke's color.
const SMOKE: [u8; 3] = [150, 140, 125];

/// The smoke at `age` frames for the GPU particle pool: the pool's particles dropped and the puffs handed over, each a sprite
/// of the puff texture facing the camera (still: the pool steps them by nothing).
fn smoke_sprites(age: Option<f32>, texture: usize) -> SpriteFrame {
    let look = SpriteLook {
        label: "smoke".into(),
        colors: [[1.0; 4]; 4],
        sizes: [1.0; 4],
        times: [0.0, 1.0, 2.0, 2.0],
        drag: 0.0,
        gravity: 0.0,
        wind: 0.0,
        blend: Blend::Alpha,
        frames: vec![texture],
        fps: 0,
        hidden: false,
        orient: torque_render_scene::particles::Orient::default(),
        lit: torque_render_scene::particles::Lit::default(),
        soft_distance: 0.4,
        flipbook_blend: false,
        size_by_velocity: 0.0,
        turbulence: 0.0,
        turbulence_scale: 1.0,
        collide: false,
        bounce_elasticity: 0.5,
        bounce_friction: 0.2,
    };
    let spawns = age
        .map(puffs)
        .unwrap_or_default()
        .into_iter()
        .map(|(c, half, alpha)| GpuSpawn {
            pos: c,
            age_ms: 0,
            vel: [0.0; 3],
            life_ms: 1 << 20,
            acc: [0.0; 3],
            spin: 0.0,
            look: 0,
            fresh: false,
            color: [
                f32::from(SMOKE[0]) / 255.0,
                f32::from(SMOKE[1]) / 255.0,
                f32::from(SMOKE[2]) / 255.0,
                f32::from((alpha * 255.0) as u8) / 255.0,
            ],
            size: 2.0 * half,
            orient_dir: [0.0, 0.0, 1.0],
        })
        .collect();
    SpriteFrame {
        looks: vec![look],
        spawns,
        steps: Vec::new(),
        clear: true,
        wind: [0.0; 3],
    }
}

/// The smoke at `age` frames: puffs rising and spreading from behind the buggy, each at its eye distance.
fn smoke(age: f32, eye: [f32; 3], texture: usize) -> Vec<(f32, HdDraw)> {
    let mut out = Vec::new();
    for (c, half, alpha) in puffs(age) {
        // Faced to the eye about the vertical.
        let (fx, fy) = (c[0] - eye[0], c[1] - eye[1]);
        let l = fx.hypot(fy).max(1e-3);
        let side = [-fy / l * half, fx / l * half];
        let corner = |a: f32, b: f32, u: f32, v: f32| Vertex {
            position: [c[0] + a * side[0], c[1] + a * side[1], c[2] + b * half],
            normal: [0.0, 0.0, 1.0],
            color: [SMOKE[0], SMOKE[1], SMOKE[2], (alpha * 255.0) as u8],
            uv: [u, v],
        };
        let (a, b, cc, d) = (
            corner(-1.0, -1.0, 0.0, 1.0),
            corner(1.0, -1.0, 1.0, 1.0),
            corner(1.0, 1.0, 1.0, 0.0),
            corner(-1.0, 1.0, 0.0, 0.0),
        );
        let draw = Draw {
            label: "smoke".into(),
            vertices: vec![a, b, cc, a, cc, d],
            lighting: false,
            tex_env: TexEnv::Modulate,
            texture: Some((0, sampler())),
            blend: Blend::Alpha,
            cull: Cull::None,
            depth_test: true,
            depth_write: false,
            fog: true,
            lights: Vec::new(),
            vertex_key: None,
        };
        let hd = HdDraw::from_fixed(&draw, material::KIND_SKY, &[texture])
            .with_material(material::PARTICLE);
        let d = [c[0] - eye[0], c[1] - eye[1], c[2] - eye[2]];
        out.push(((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt(), hd));
    }
    out.sort_by(|a, b| b.0.total_cmp(&a.0));
    out
}

fn look() -> ScatterLook {
    ScatterLook {
        shadow: [0.12, 0.13, 0.16],
        night_color: [0.11, 0.13, 0.19],
        night_fog_color: [0.04, 0.05, 0.08],
        moon_azimuth: 200.0,
        moon_elevation: 50.0,
        moon_light_color: [0.05, 0.06, 0.09],
        ..ScatterLook::default()
    }
}

/// The scene as the demo's view sets it up (the physical sky, its sun radius, occlusion and smooth occlusion), at the shot's
/// quality with its fixtures.
struct Stage {
    scene: PlainScene,
    puff: usize,
    poles: bool,
    /// The smoke through the GPU particle pool.
    sprites: bool,
    table: &'static ScatterTable,
    look: ScatterLook,
    /// The state last put in.
    last: Option<State>,
}

impl Stage {
    fn new(shot: &Shot, sprites: bool) -> Stage {
        let mut frame = HdScene::new(StaticCells::default());
        frame.atmosphere = true;
        frame.sun_radius = 0.27;
        frame.ao_radius = 1.0;
        frame.ao_sun = 0.5;
        frame.smooth_occlusion = true;
        frame.visible_distance = 400.0;
        frame.fog_distance = 150.0;
        frame.fog.start = 150.0;
        frame.fog.end = 400.0;
        let ground_tex = frame.textures.intern(dirt());
        let puff = frame.textures.intern(puff());
        frame.planes = vec![HdDraw::from_fixed(
            &ground(200.0),
            material::KIND_PLANE,
            &[ground_tex],
        )];
        if shot.water {
            frame.water = vec![HdDraw::from_fixed(&water(200.0), material::KIND_PLANE, &[])];
        }
        if shot.fog {
            frame.fog_distance = 4.0;
            frame.fog.start = 4.0;
            frame.fog.end = 90.0;
        }
        frame.size = SIZE;
        frame.fov = FOV;
        Stage {
            scene: PlainScene {
                frame,
                mode: Some(HdMode::new(shot.quality)),
                time: 0.0,
            },
            puff,
            poles: shot.poles,
            sprites,
            table: ScatterTable::earth(),
            look: look(),
            last: None,
        }
    }

    /// Puts `st` in, moved from the state last put in (`still`: no motion, the reference's frozen instant).
    fn put(&mut self, st: &State, still: bool) {
        let before = if still { Some(*st) } else { self.last };
        let s = &mut self.scene.frame;
        let eye = look_at(st.eye, st.target);
        s.eye = Some(eye);
        if self.last.is_none_or(|l| l.sun != st.sun) {
            let [az, el] = st.sun.map(f32::to_radians);
            let sun = [el.cos() * az.sin(), el.cos() * az.cos(), el.sin()];
            let light = self.table.light(sun, &self.look);
            s.sun = light.sun();
            if s.sun.shadow == [0.0; 3] {
                s.sun.shadow = [0.35, 0.38, 0.45];
            }
            s.fog = light.fog(s.fog);
            s.clear = light.fog;
            s.atmosphere_sun = Some(sun);
            s.clouds = Some(light.clouds([1.0, 0.4, 0.0]));
        }
        let mut objects = blocks();
        if self.poles {
            objects.extend(poles());
        }
        if let Some(at) = st.buggy {
            let from = before.and_then(|b| b.buggy).unwrap_or(at);
            objects.push(buggy(at, from));
        }
        s.objects = objects;
        if self.last.is_none_or(|l| l.light != st.light) {
            s.lights = if st.light {
                vec![HdLight {
                    position: [-2.5, -1.0, 1.2],
                    color: [3.0, 2.4, 1.6],
                    radius: 9.0,
                    carrier: None,
                }]
            } else {
                Vec::new()
            };
            s.lights_generation += 1;
        }
        if self
            .last
            .is_none_or(|l| l.headlights != st.headlights || l.buggy != st.buggy)
        {
            s.spots = match (st.headlights, st.buggy) {
                (true, Some(at)) => [-0.5, 0.5].map(|dy| headlight(at, dy)).to_vec(),
                _ => Vec::new(),
            };
            s.lights_generation += 1;
        }
        if self
            .last
            .is_none_or(|l| l.smoke != st.smoke || l.eye != st.eye)
        {
            if self.sprites {
                s.push_sprites(&smoke_sprites(st.smoke, self.puff));
            } else {
                s.particles = st
                    .smoke
                    .map(|age| smoke(age, st.eye, self.puff))
                    .unwrap_or_default();
                s.particles_generation += 1;
            }
        }
        s.generation += 1;
        self.last = Some(*st);
    }

    fn camera(&self, st: &State) -> HdCamera {
        HdCamera::new(
            look_at(st.eye, st.target),
            FOV,
            self.scene.visible_distance,
            SIZE.0,
            SIZE.1,
        )
    }
}

/// A world drawing into an offscreen target.
struct Drawer {
    world: HdWorld<NoExtension>,
    target: torque_render_core::Offscreen,
}

impl Drawer {
    fn new(gpu: &Gpu, sprites: bool) -> Result<Drawer, String> {
        let mut world = HdWorld::new(gpu);
        world.features.gpu_particles = if sprites {
            ParticlePool::On
        } else {
            ParticlePool::Off
        };
        Ok(Drawer {
            world,
            target: gpu.offscreen(SIZE.0, SIZE.1).map_err(|e| e.to_string())?,
        })
    }

    fn draw(&mut self, stage: &Stage, camera: &HdCamera) -> Result<(), String> {
        let format = self.target.color.texture().format();
        self.world.draw(
            &stage.scene,
            camera,
            Some((&self.target.color, format, [0, 0])),
        )
    }

    fn read(&self, gpu: &Gpu) -> Result<Image, String> {
        Ok(ghosting::image(
            &gpu.read_back(&self.target).map_err(|e| e.to_string())?,
        ))
    }
}

/// The references of `shot`, one a frame (a frame whose state is the last one's shares its image).
fn references(
    gpu: &Gpu,
    shot: &Shot,
    sprites: bool,
    march: bool,
) -> Result<Vec<Arc<Image>>, String> {
    let mut stage = Stage::new(shot, sprites);
    let mut d = Drawer::new(gpu, sprites)?;
    d.world.features.froxel_fog = !march;
    let mut out: Vec<Arc<Image>> = Vec::with_capacity(shot.frames);
    let mut last: Option<State> = None;
    for f in 0..shot.frames {
        let st = (shot.state)(f);
        if last == Some(st) {
            let same = out.last().cloned().ok_or("no reference")?;
            out.push(same);
            continue;
        }
        stage.scene.time = f as f32 / 60.0;
        stage.put(&st, true);
        d.world.upload(&stage.scene);
        let camera = stage.camera(&st);
        let img = ghosting::settled(gpu, &mut d.world, &stage.scene, &camera, &d.target)?;
        out.push(Arc::new(img));
        last = Some(st);
    }
    Ok(out)
}

fn write_png(path: &Path, img: &Image) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), SIZE.0, SIZE.1);
    enc.set_color(png::ColorType::Rgb);
    enc.write_header()
        .and_then(|mut w| w.write_image_data(&ghosting::bytes(img)))
        .map_err(|e| e.to_string())
}

struct Args {
    audit: bool,
    trace: bool,
    frame: Option<usize>,
    shot: Option<String>,
    out: Option<PathBuf>,
    max_settle: Option<usize>,
    max_trail: Option<f64>,
    gpu_particles: bool,
    quality: Option<Quality>,
    march: bool,
}

fn args() -> Result<Args, String> {
    let mut a = Args {
        audit: false,
        trace: false,
        frame: None,
        shot: None,
        out: None,
        max_settle: Some(MAX_SETTLE),
        max_trail: Some(MAX_TRAIL),
        gpu_particles: false,
        quality: None,
        march: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or(format!("{arg} wants a value"));
        match arg.as_str() {
            "--audit" => a.audit = true,
            "--trace" => a.trace = true,
            "--gpu-particles" => a.gpu_particles = true,
            "--march" => a.march = true,
            "--frame" => a.frame = Some(value()?.parse().map_err(|e| format!("{e}"))?),
            "--shot" => a.shot = Some(value()?),
            "--out" => a.out = Some(PathBuf::from(value()?)),
            "--max-settle" => a.max_settle = Some(value()?.parse().map_err(|e| format!("{e}"))?),
            "--max-trail" => a.max_trail = Some(value()?.parse().map_err(|e| format!("{e}"))?),
            "--quality" => {
                let v = value()?;
                a.quality = Some(Quality::named(&v).ok_or(format!("no quality {v}"))?);
            }
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    Ok(a)
}

fn main() -> Result<(), String> {
    let args = args()?;
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    if let Some(dir) = &args.out {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    println!(
        "a pixel is off past {TOLERANCE}/255 a channel; a frame is within at {:.1} % off or less; settle in frames",
        AREA_OK * 100.0
    );
    println!("{}", Measure::HEADER);
    let mut failed = Vec::new();
    for mut shot in shots() {
        if let Some(q) = args.quality {
            shot.quality = q;
        }
        if args.shot.as_deref().is_some_and(|s| s != shot.name) {
            continue;
        }
        let refs = references(&gpu, &shot, args.gpu_particles, args.march)?;
        for (name, set) in ghosting::settings::<NoExtension>(args.audit) {
            let mut stage = Stage::new(&shot, args.gpu_particles);
            let mut d = Drawer::new(&gpu, args.gpu_particles)?;
            d.world.features.froxel_fog = !args.march;
            set(&mut d.world);
            let mut areas = Vec::with_capacity(shot.frames);
            let mut frames: Vec<Image> = Vec::new();
            // The GPU time a frame: the whole frame, the sun mask's pass, the anti-aliasing's; the frames timed.
            let mut gpu_ms = [0.0f64; 3];
            let mut timed = 0usize;
            for (f, reference) in refs.iter().enumerate() {
                let st = (shot.state)(f);
                stage.scene.time = f as f32 / 60.0;
                stage.put(&st, false);
                d.world.upload(&stage.scene);
                d.draw(&stage, &stage.camera(&st))?;
                let img = d.read(&gpu)?;
                if let Some(t) = d.world.gpu_ms() {
                    let g = d.world.gpu_times().unwrap_or_default();
                    gpu_ms[0] += t;
                    gpu_ms[1] += g.contact;
                    gpu_ms[2] += g.aa;
                    timed += 1;
                }
                areas.push(ghosting::share_off(&img, reference));
                if args.out.is_some() {
                    frames.push(img);
                }
            }
            let m = Measure::of(&areas, &shot.changes);
            let gpu_note = if timed > 0 {
                let [f, c, a] = gpu_ms.map(|v| v / timed as f64);
                format!("  gpu {f:.3} ms (mask {c:.3}, aa {a:.3})")
            } else {
                String::new()
            };
            println!("{}{gpu_note}", m.row(shot.name, name));
            if args.trace {
                let shares: Vec<String> =
                    areas.iter().map(|a| format!("{:.2}", a * 100.0)).collect();
                println!("  frames (% off): {}", shares.join(" "));
            }
            if let Some(dir) = &args.out {
                let stem = format!("{}-{}", shot.name, name.replace(' ', "-"));
                let picked = args.frame.filter(|&f| f < frames.len() && f != m.worst_at);
                for at in [m.worst_at].into_iter().chain(picked) {
                    write_png(&dir.join(format!("{stem}-{at}.png")), &frames[at])?;
                    write_png(&dir.join(format!("{stem}-{at}-ref.png")), &refs[at])?;
                    write_png(
                        &dir.join(format!("{stem}-{at}-diff.png")),
                        &ghosting::difference(&frames[at], &refs[at]),
                    )?;
                }
            }
            if name == "default" && shot.gated {
                if args.max_settle.is_some_and(|x| m.settle > x) {
                    failed.push(format!("{}: settles in {} frames", shot.name, m.settle));
                }
                if args.max_trail.is_some_and(|x| m.trail * 100.0 > x) {
                    failed.push(format!("{}: trail {:.2} %", shot.name, m.trail * 100.0));
                }
            }
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("; "))
    }
}
