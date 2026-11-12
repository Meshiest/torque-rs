//! The GPU particle pool against the CPU path: the same emitters (smoke blended by its alpha, sparks added) run for 120
//! frames (every third frame two steps of the simulation), their particles handed to the pool each frame or drawn as the CPU
//! path's quads. The pool keeps exactly the
//! particles the CPU path keeps, its pictures are the same byte for byte from run to run, and they are the CPU path's within a
//! few levels.

use std::sync::Arc;

use torque_render_core::{Frame, Gpu};
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::hd::{HdCamera, HdMode, Quality, material};
use torque_render_scene::particles::{
    Emitter, EmitterData, GpuSpawn, MAX_PARTICLES, ParticleData, Rng, SpriteFrame, SpriteLook,
};
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Assets, Blend, Texture};

const SIZE: (u32, u32) = (320, 180);
const FRAMES: u32 = 120;
const MS: u32 = 16;

struct NoFiles;
impl Assets for NoFiles {
    fn read(&self, _: &str) -> Option<Vec<u8>> {
        None
    }
}

fn particle(inv_alpha: bool) -> ParticleData {
    ParticleData {
        drag: if inv_alpha { 0.6 } else { 0.1 },
        wind: 0.0,
        gravity: if inv_alpha { -0.15 } else { 0.8 },
        inherited_vel: 0.0,
        constant_accel: 0.0,
        lifetime_ms: if inv_alpha { 1500 } else { 700 },
        lifetime_variance_ms: 200,
        spin_speed: 40.0,
        spin_min: -30.0,
        spin_max: 30.0,
        inv_alpha,
        colors: if inv_alpha {
            [
                [0.55, 0.52, 0.5, 0.0],
                [0.55, 0.52, 0.5, 0.7],
                [0.45, 0.45, 0.45, 0.3],
                [0.4, 0.4, 0.4, 0.0],
            ]
        } else {
            [
                [1.0, 0.8, 0.3, 1.0],
                [1.0, 0.5, 0.1, 1.0],
                [0.8, 0.2, 0.0, 0.5],
                [0.5, 0.1, 0.0, 0.0],
            ]
        },
        sizes: if inv_alpha {
            [0.4, 0.9, 1.4, 1.8]
        } else {
            [0.12, 0.1, 0.08, 0.05]
        },
        times: [0.0, 0.2, 0.6, 1.0],
        textures: Vec::new(),
        animate: false,
        fps: 0,
    }
}

fn emitter(inv_alpha: bool) -> Emitter {
    let data = EmitterData {
        period_ms: if inv_alpha { 12 } else { 4 },
        period_variance_ms: 2,
        velocity: if inv_alpha { 1.2 } else { 5.0 },
        velocity_variance: 0.6,
        offset: 0.05,
        theta_min: 0.0,
        theta_max: if inv_alpha { 25.0 } else { 50.0 },
        phi_ref_vel: 0.0,
        phi_variance: 360.0,
        override_advance: false,
        lifetime_ms: 0,
        use_emitter_sizes: false,
        use_emitter_colors: false,
        particles: vec![1],
    };
    Emitter::of_data(data, vec![particle(inv_alpha)]).expect("an emitter")
}

fn scene() -> PlainScene {
    let mut s = PlainScene {
        mode: Some(HdMode::new(Quality::High)),
        ..PlainScene::default()
    };
    s.frame.sun.towards = [-0.3, -0.4, 0.86];
    s.frame.sun.diffuse = [0.8; 3];
    s.frame.sun.ambient = [0.35; 3];
    let mut floor = box_object(0, [0.0, 4.0, -0.5], [20.0, 20.0, 0.5])
        .draws
        .remove(0);
    floor.label = "floor".into();
    for v in &mut floor.vertices {
        v.material = material::KIND_PLANE << material::KIND_SHIFT;
    }
    s.frame.planes = vec![floor];
    s.frame.planes_generation += 1;
    s.frame.eye = Some(look_at([0.0, -4.0, 2.0], [0.0, 4.0, 1.5]));
    s.frame.fov = 70;
    s.frame.size = SIZE;
    s
}

/// What a run leaves: the last frame's picture and the particles alive (the CPU path's, or the pool's).
struct Run {
    picture: Frame,
    live: u32,
}

/// The emitters run `FRAMES` frames, drawn through the pool (`gpu`) or the CPU path. None: no GPU, or one that does not run
/// the pool.
fn run(gpu: &Gpu, pool: bool) -> Option<Run> {
    let mut s = scene();
    let eye = s.frame.eye.expect("the eye");
    let camera = HdCamera::new(eye, f32::from(s.frame.fov), 200.0, SIZE.0, SIZE.1);
    let right = [camera.view[0], camera.view[4], camera.view[8]];
    let up = [camera.view[1], camera.view[5], camera.view[9]];
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    world.features.gpu_particles = pool;
    if pool && !world.gpu_particles_on() {
        return None;
    }
    let mut emitters = [(emitter(true), "smoke"), (emitter(false), "sparks")];
    let mut rng = Rng::new(0x5eed);
    let mut live = 0usize;
    s.frame.generation += 1;
    world.upload(&s);
    for k in 0..FRAMES {
        let t = k as f32 * 0.02;
        let steps = if k % 3 == 0 { 2 } else { 1 };
        let mut frame = SpriteFrame {
            steps: vec![MS; steps],
            ..SpriteFrame::default()
        };
        let mut set = TextureSet::new(&NoFiles);
        let mut draws = Vec::new();
        for (i, (e, label)) in emitters.iter_mut().enumerate() {
            let at = [i as f32 * 1.5 - 0.75 + t.sin() * 0.3, 4.0, 0.2];
            for _ in 0..steps {
                e.advance(MS, &mut live);
                e.emit_path(at, [0.0, 0.0, 1.0], [0.0; 3], MS, &mut rng, &mut live);
            }
            if pool {
                e.hand_over(&mut frame, &mut set, label, &mut live);
            } else if let Some(d) = e.draw(right, up, &mut set, label) {
                draws.push((10.0 - i as f32, d));
            }
        }
        s.frame
            .set_particles_decals_and_sprites(&draws, &[], pool.then_some(&frame), set);
        s.time = k as f32 * MS as f32 / 1000.0;
        world.draw(&s, &camera, None).expect("a frame");
    }
    let live = if pool {
        world
            .gpu_particle_counts()
            .expect("the counts")
            .expect("a pool")
            .live()
    } else {
        emitters.iter().map(|(e, _)| e.particles.len() as u32).sum()
    };
    Some(Run {
        picture: world.read_ldr().expect("the picture"),
        live,
    })
}

fn gpu() -> Option<Gpu> {
    match Gpu::new() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipped: no GPU ({e})");
            None
        }
    }
}

#[test]
fn the_pool_draws_the_same_picture_every_run() {
    let Some(gpu) = gpu() else { return };
    let (Some(a), Some(b)) = (run(&gpu, true), run(&gpu, true)) else {
        eprintln!("skipped: the device does not run the pool");
        return;
    };
    assert!(a.live > 100, "{} particles live", a.live);
    assert_eq!(a.live, b.live);
    assert!(
        a.picture.rgba == b.picture.rgba,
        "the two runs' pictures differ"
    );
}

#[test]
fn the_pool_keeps_the_cpu_paths_particles_and_draws_them_as_it_does() {
    let Some(gpu) = gpu() else { return };
    let Some(pool) = run(&gpu, true) else {
        eprintln!("skipped: the device does not run the pool");
        return;
    };
    let cpu = run(&gpu, false).expect("the CPU path");
    assert_eq!(
        pool.live, cpu.live,
        "the particles alive after {FRAMES} frames"
    );
    // Within a few levels where the particles are (the pool's blend order is its slot order, the CPU path's its emitters').
    let (a, b) = (&pool.picture.rgba, &cpu.picture.rgba);
    let off = a
        .chunks(4)
        .zip(b.chunks(4))
        .filter(|(p, q)| p.iter().zip(*q).take(3).any(|(x, y)| x.abs_diff(*y) > 4))
        .count();
    let share = off as f64 / (a.len() / 4) as f64;
    let blank = run_blank(&gpu);
    let drawn = b
        .chunks(4)
        .zip(blank.rgba.chunks(4))
        .filter(|(p, q)| p.iter().zip(*q).take(3).any(|(x, y)| x.abs_diff(*y) > 4))
        .count() as f64
        / (b.len() / 4) as f64;
    assert!(
        drawn > 0.02,
        "the particles cover {:.2} % of the picture",
        drawn * 100.0
    );
    assert!(
        share < 0.002,
        "{:.3} % of the pixels differ by more than 4 levels (the particles cover {:.2} %)",
        share * 100.0,
        drawn * 100.0
    );
}

/// The scene drawn with no particles (what the particles cover is what differs from it).
fn run_blank(gpu: &Gpu) -> Frame {
    let s = scene();
    let eye = s.frame.eye.expect("the eye");
    let camera = HdCamera::new(eye, f32::from(s.frame.fov), 200.0, SIZE.0, SIZE.1);
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    let mut s = s;
    s.frame.generation += 1;
    world.upload(&s);
    for k in 0..FRAMES {
        s.time = k as f32 * MS as f32 / 1000.0;
        world.draw(&s, &camera, None).expect("a frame");
    }
    world.read_ldr().expect("the picture")
}

#[test]
fn a_blended_spawn_draws_in_its_bucket_and_a_clear_empties_the_pool() {
    let Some(gpu) = gpu() else { return };
    let mut s = scene();
    let eye = s.frame.eye.expect("the eye");
    let camera = HdCamera::new(eye, f32::from(s.frame.fov), 200.0, SIZE.0, SIZE.1);
    let mut world: HdWorld<NoExtension> = HdWorld::new(&gpu);
    world.features.gpu_particles = true;
    if !world.gpu_particles_on() {
        return;
    }
    let mut e = emitter(true);
    let (mut rng, mut live) = (Rng::new(4), 0);
    let mut frame = SpriteFrame::default();
    let mut set = TextureSet::new(&NoFiles);
    e.emit_path(
        [0.0, 4.0, 1.0],
        [0.0, 0.0, 1.0],
        [0.0; 3],
        120,
        &mut rng,
        &mut live,
    );
    let made = e.particles.len() as u32;
    e.hand_over(&mut frame, &mut set, "smoke", &mut live);
    s.frame.push_sprites(&frame);
    assert_eq!(s.frame.sprites.buckets.len(), 1);
    assert_eq!(s.frame.sprites.buckets[0].blend, Blend::Alpha);
    s.frame.generation += 1;
    world.upload(&s);
    world.draw(&s, &camera, None).expect("a frame");
    let counts = world.gpu_particle_counts().unwrap().unwrap();
    assert_eq!(counts.buckets, vec![made]);
    // The pool holds no more than the CPU path's cap by default.
    assert_eq!(counts.free, MAX_PARTICLES as u32 - made);
    assert_eq!(counts.dropped, 0);
    // Drawn again, the same frame steps and spawns nothing.
    world.draw(&s, &camera, None).expect("a frame");
    assert_eq!(
        world.gpu_particle_counts().unwrap().unwrap().buckets,
        vec![made]
    );
    s.frame.push_sprites(&SpriteFrame {
        clear: true,
        ..SpriteFrame::default()
    });
    world.draw(&s, &camera, None).expect("a frame");
    assert_eq!(world.gpu_particle_counts().unwrap().unwrap().live(), 0);
}

/// A particle of `look` at `x` along the floor, long-lived and still.
fn still(look: u32, x: f32) -> GpuSpawn {
    GpuSpawn {
        pos: [x, 4.0, 1.0],
        age_ms: 0,
        vel: [0.0; 3],
        life_ms: 60_000,
        acc: [0.0; 3],
        spin: 0.0,
        look,
        fresh: false,
        color: [1.0, 0.6, 0.2, 0.8],
        size: 0.3,
    }
}

fn look_of(frames: Vec<usize>) -> SpriteLook {
    SpriteLook {
        label: "x".into(),
        colors: [[1.0; 4]; 4],
        sizes: [0.3; 4],
        times: [0.0, 1.0, 2.0, 2.0],
        drag: 0.0,
        gravity: 0.0,
        wind: 0.0,
        blend: Blend::Alpha,
        frames,
        fps: 0,
        hidden: false,
    }
}

/// A pool held to `cap` particles takes `frames` of spawns (each its count) and draws a few frames after each: the particles
/// live, the spawns dropped and the picture.
fn small_pool(gpu: &Gpu, cap: u32, frames: &[u32]) -> Option<(u32, u64, Vec<u8>)> {
    let mut s = scene();
    let eye = s.frame.eye.expect("the eye");
    let camera = HdCamera::new(eye, f32::from(s.frame.fov), 200.0, SIZE.0, SIZE.1);
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    world.features.gpu_particles = true;
    world.set_gpu_particle_cap(cap);
    if !world.gpu_particles_on() {
        return None;
    }
    s.frame.generation += 1;
    world.upload(&s);
    let mut at = 0;
    for &n in frames {
        s.frame.push_sprites(&SpriteFrame {
            looks: vec![look_of(Vec::new())],
            spawns: (at..at + n)
                .map(|k| still(0, (k % 200) as f32 * 0.05 - 5.0))
                .collect(),
            ..SpriteFrame::default()
        });
        at += n;
        for _ in 0..3 {
            world.draw(&s, &camera, None).expect("a frame");
        }
    }
    let counts = world.gpu_particle_counts().expect("the counts")?;
    let picture = world.read_ldr().expect("the picture").rgba;
    Some((counts.live(), counts.dropped, picture))
}

#[test]
fn spawns_past_the_pools_free_slots_are_dropped_and_counted_the_same_every_run() {
    let Some(gpu) = gpu() else { return };
    // 40 fit; 30 of the next 40 do not; of 200, 150 are past the pool and the other 50 find no free slot.
    let frames = [40, 40, 200];
    let Some(a) = small_pool(&gpu, 50, &frames) else {
        return;
    };
    assert_eq!((a.0, a.1), (50, 30 + 150 + 50));
    let b = small_pool(&gpu, 50, &frames).expect("the pool again");
    assert_eq!((a.0, a.1), (b.0, b.1));
    assert!(a.2 == b.2, "the two runs' pictures differ");
}

#[test]
fn looks_past_the_buckets_are_hidden_and_the_rest_drawn() {
    let Some(gpu) = gpu() else { return };
    let mut s = scene();
    let eye = s.frame.eye.expect("the eye");
    let camera = HdCamera::new(eye, f32::from(s.frame.fov), 200.0, SIZE.0, SIZE.1);
    let mut world: HdWorld<NoExtension> = HdWorld::new(&gpu);
    world.features.gpu_particles = true;
    if !world.gpu_particles_on() {
        return;
    }
    // 80 textures, a look and a particle each.
    let looks: Vec<SpriteLook> = (0..80u8)
        .map(|k| {
            let t = s.frame.textures.intern(Texture {
                path: format!("tint{k}"),
                width: 1,
                height: 1,
                levels: Arc::new(vec![vec![k * 3, 255 - k * 3, 128, 255]]),
            });
            look_of(vec![t])
        })
        .collect();
    let spawns = (0..80u32).map(|k| still(k, k as f32 * 0.1 - 4.0)).collect();
    s.frame.push_sprites(&SpriteFrame {
        looks,
        spawns,
        ..SpriteFrame::default()
    });
    let sprites = &s.frame.sprites;
    assert_eq!(sprites.buckets.len(), 64);
    assert!(sprites.overflow > 0);
    assert_eq!(sprites.live_buckets().count(), 63);
    s.frame.generation += 1;
    world.upload(&s);
    world.draw(&s, &camera, None).expect("a frame");
    let counts = world.gpu_particle_counts().unwrap().unwrap();
    // Every particle lives, the hidden ones in the hidden bucket (not drawn).
    assert_eq!(counts.buckets.len(), 64);
    assert_eq!(counts.live(), 80);
    let hidden = sprites
        .buckets
        .iter()
        .position(|b| !b.drawn)
        .expect("the hidden bucket");
    assert_eq!(counts.buckets[hidden], 80 - 63);
}
