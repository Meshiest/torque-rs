//! The GPU particle pool against the CPU path: the same emitters (smoke blended by its alpha, sparks added) run for 120
//! frames (every third frame two steps of the simulation), their particles handed to the pool each frame or drawn as the CPU
//! path's quads. The pool keeps exactly the
//! particles the CPU path keeps, its pictures are the same byte for byte from run to run, and they are the CPU path's within a
//! few levels.

use std::f32::consts::FRAC_1_SQRT_2;
use std::sync::Arc;

use torque_render_core::{Frame, Gpu};
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_hd::{HdWorld, ParticlePool};
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::hd::particle_light::ParticleLight;
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
        wind: 1.0,
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
        extras: torque_render_scene::particles::ParticleExtras::default(),
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
        extras: torque_render_scene::particles::EmitterExtras::default(),
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
fn run(gpu: &Gpu, pool: bool, wind: [f32; 3]) -> Option<Run> {
    run_with(gpu, pool, wind, false)
}

/// [`run`] with the smoke lit (`lit`: the usual rule for blended particles) or not.
fn run_with(gpu: &Gpu, pool: bool, wind: [f32; 3], lit: bool) -> Option<Run> {
    let mut s = scene();
    if lit {
        s.frame.particle_light = Some(ParticleLight::BLENDED);
    }
    let eye = s.frame.eye.expect("the eye");
    let camera = HdCamera::new(eye, f32::from(s.frame.fov), 200.0, SIZE.0, SIZE.1);
    let right = [camera.view[0], camera.view[4], camera.view[8]];
    let up = [camera.view[1], camera.view[5], camera.view[9]];
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    world.features.gpu_particles = if pool {
        ParticlePool::On
    } else {
        ParticlePool::Off
    };
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
            wind,
            ..SpriteFrame::default()
        };
        let mut set = TextureSet::new(&NoFiles);
        let mut draws = Vec::new();
        for (i, (e, label)) in emitters.iter_mut().enumerate() {
            let at = [i as f32 * 1.5 - 0.75 + t.sin() * 0.3, 4.0, 0.2];
            e.set_wind(wind);
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
    let (Some(a), Some(b)) = (run(&gpu, true, [0.0; 3]), run(&gpu, true, [0.0; 3])) else {
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
    let Some(pool) = run(&gpu, true, [0.0; 3]) else {
        eprintln!("skipped: the device does not run the pool");
        return;
    };
    let cpu = run(&gpu, false, [0.0; 3]).expect("the CPU path");
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
    world.features.gpu_particles = ParticlePool::On;
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
        orient_dir: [0.0, 0.0, 1.0],
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
        orient: torque_render_scene::particles::Orient::default(),
        lit: torque_render_scene::particles::Lit::default(),
        soft_distance: 0.4,
        flipbook_blend: false,
        size_by_velocity: 0.0,
    }
}

/// A pool held to `cap` particles takes `frames` of spawns (each its count) and draws a few frames after each: the particles
/// live, the spawns dropped and the picture.
fn small_pool(gpu: &Gpu, cap: u32, frames: &[u32]) -> Option<(u32, u64, Vec<u8>)> {
    let mut s = scene();
    let eye = s.frame.eye.expect("the eye");
    let camera = HdCamera::new(eye, f32::from(s.frame.fov), 200.0, SIZE.0, SIZE.1);
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    world.features.gpu_particles = ParticlePool::On;
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
    world.features.gpu_particles = ParticlePool::On;
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

/// The share of the pixels that differ by more than 4 levels in a color channel.
fn share_differing(a: &[u8], b: &[u8]) -> f64 {
    let off = a
        .chunks(4)
        .zip(b.chunks(4))
        .filter(|(p, q)| p.iter().zip(*q).take(3).any(|(x, y)| x.abs_diff(*y) > 4))
        .count();
    off as f64 / (a.len() / 4) as f64
}

#[test]
fn the_pool_carries_its_particles_by_the_wind_as_the_cpu_path_does() {
    let Some(gpu) = gpu() else { return };
    let wind = [1.5, 0.0, 0.0];
    let Some(pool) = run(&gpu, true, wind) else {
        eprintln!("skipped: the device does not run the pool");
        return;
    };
    let cpu = run(&gpu, false, wind).expect("the CPU path");
    assert_eq!(pool.live, cpu.live);
    assert!(
        share_differing(&pool.picture.rgba, &cpu.picture.rgba) < 0.002,
        "the pool's picture is not the CPU path's under a wind"
    );
    // The wind moved them: the windless picture is not this one.
    let still = run(&gpu, true, [0.0; 3]).expect("the pool");
    assert!(
        share_differing(&pool.picture.rgba, &still.picture.rgba) > 0.01,
        "the wind did not move the particles"
    );
}

/// `scene` drawn for a few frames with one particle of `look` in it, and again with none: the pixels the particle changes,
/// as the width and height of their box and their count. None: no pool.
fn covered(
    gpu: &Gpu,
    scene: &PlainScene,
    look: SpriteLook,
    spawn: GpuSpawn,
) -> Option<(u32, u32, usize)> {
    let draw = |sprites: Option<SpriteFrame>| {
        let mut s = scene.clone();
        let eye = s.frame.eye.expect("the eye");
        let camera = HdCamera::new(eye, f32::from(s.frame.fov), 200.0, SIZE.0, SIZE.1);
        let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
        world.features.gpu_particles = ParticlePool::On;
        if !world.gpu_particles_on() {
            return None;
        }
        s.frame.generation += 1;
        world.upload(&s);
        if let Some(frame) = sprites {
            s.frame.push_sprites(&frame);
        }
        for _ in 0..4 {
            world.draw(&s, &camera, None).expect("a frame");
        }
        Some(world.read_ldr().expect("the picture").rgba)
    };
    let drawn = draw(Some(SpriteFrame {
        looks: vec![look],
        spawns: vec![spawn],
        ..SpriteFrame::default()
    }))?;
    let blank = draw(None)?;
    let (mut lo, mut hi, mut n) = ((u32::MAX, u32::MAX), (0, 0), 0);
    for (i, (p, q)) in drawn.chunks(4).zip(blank.chunks(4)).enumerate() {
        if p.iter().zip(q).take(3).any(|(x, y)| x.abs_diff(*y) > 12) {
            let (x, y) = (i as u32 % SIZE.0, i as u32 / SIZE.0);
            lo = (lo.0.min(x), lo.1.min(y));
            hi = (hi.0.max(x), hi.1.max(y));
            n += 1;
        }
    }
    Some(if n == 0 {
        (0, 0, 0)
    } else {
        (hi.0 - lo.0 + 1, hi.1 - lo.1 + 1, n)
    })
}

/// The box of the pixels a single particle at the middle of the floor scene covers.
fn extent(gpu: &Gpu, look: SpriteLook, spawn: GpuSpawn) -> Option<(u32, u32)> {
    covered(gpu, &scene(), look, spawn).map(|(w, h, _)| (w, h))
}

/// The scene with nothing in it but the sky.
fn sky_scene() -> PlainScene {
    let mut s = scene();
    s.frame.planes.clear();
    s.frame.planes_generation += 1;
    s.frame.eye = Some(look_at([0.0, -4.0, 2.0], [0.0, 4.0, 3.0]));
    s
}

#[test]
fn a_sprite_over_bare_sky_draws_blended_and_added() {
    let Some(gpu) = gpu() else { return };
    for blend in [Blend::Alpha, Blend::Additive] {
        let look = SpriteLook {
            blend,
            ..look_of(Vec::new())
        };
        let spawn = GpuSpawn {
            pos: [0.0, 4.0, 2.5],
            size: 1.0,
            color: [1.0, 0.1, 0.1, 1.0],
            ..still(0, 0.0)
        };
        let Some((w, h, n)) = covered(&gpu, &sky_scene(), look, spawn) else {
            eprintln!("skipped: the device does not run the pool");
            return;
        };
        assert!(
            w >= 10 && h >= 10 && n >= 80,
            "{blend:?}: a sprite over bare sky covers {w} x {h} ({n} pixels)"
        );
    }
}

#[test]
fn an_oriented_particle_lies_along_its_direction_and_one_at_rest_has_no_velocity_to_lie_along() {
    use torque_render_scene::particles::Orient;
    let Some(gpu) = gpu() else { return };
    let diagonal = [FRAC_1_SQRT_2, 0.0, FRAC_1_SQRT_2];
    let mut spawn = GpuSpawn {
        pos: [0.0, 4.0, 1.0],
        size: 1.0,
        color: [1.0, 0.1, 0.1, 1.0],
        orient_dir: diagonal,
        ..still(0, 0.0)
    };
    let look = |orient| SpriteLook {
        orient,
        ..look_of(Vec::new())
    };
    let Some((w, h)) = extent(&gpu, look(Orient::Billboard), spawn) else {
        eprintln!("skipped: the device does not run the pool");
        return;
    };
    assert!(w >= 10 && h >= 10, "a billboard covers {w} x {h}");
    assert!(w.abs_diff(h) <= 2, "a billboard is square: {w} x {h}");
    // A square turned an eighth of a turn about the line of sight has a box 1.41 times as wide and as high.
    let (ew, eh) = extent(&gpu, look(Orient::Emission), spawn).expect("the pool");
    let (a, b) = (f64::from(ew) / f64::from(w), f64::from(eh) / f64::from(h));
    assert!(
        (1.25..1.6).contains(&a) && (1.25..1.6).contains(&b),
        "billboard {w} x {h}, along its emission direction {ew} x {eh}"
    );
    // Along its velocity: the same square turned to it; at rest it draws nothing.
    spawn.vel = [2.0, 0.0, 2.0];
    let (vw, vh) = extent(&gpu, look(Orient::Velocity), spawn).expect("the pool");
    assert!(vw.abs_diff(ew) <= 2 && vh.abs_diff(eh) <= 2, "{vw} x {vh}");
    spawn.vel = [0.0; 3];
    assert_eq!(extent(&gpu, look(Orient::Velocity), spawn), Some((0, 0)));
}

#[test]
fn a_lit_pool_sprite_takes_its_suns_visibility_from_its_vertex_stage_as_the_cpu_path_does_per_pixel()
 {
    let Some(gpu) = gpu() else { return };
    let Some(pool) = run_with(&gpu, true, [0.0; 3], true) else {
        eprintln!("skipped: the device does not run the pool");
        return;
    };
    let cpu = run_with(&gpu, false, [0.0; 3], true).expect("the CPU path");
    assert_eq!(pool.live, cpu.live);
    let unlit = run_with(&gpu, true, [0.0; 3], false).expect("the pool");
    assert!(
        share_differing(&pool.picture.rgba, &unlit.picture.rgba) > 0.01,
        "lighting the smoke did not change its picture"
    );
    assert!(
        share_differing(&pool.picture.rgba, &cpu.picture.rgba) < 0.002,
        "the lit pool's picture is not the lit CPU path's"
    );
}

/// A table texture of one color, `size` texels square with a mip chain (the atlas builds from level 0 and below).
fn solid_texture(size: u32, rgba: [u8; 4]) -> Texture {
    let mut levels = Vec::new();
    let mut side = size;
    loop {
        levels.push(rgba.repeat((side * side) as usize));
        if side == 1 {
            break;
        }
        side /= 2;
    }
    Texture {
        path: format!("solid{rgba:?}"),
        width: size,
        height: size,
        levels: Arc::new(levels),
    }
}

/// The color at the middle of the box one particle changes (the particle's center), drawn over the floor scene with the
/// scene's table holding `textures` (the look's frames name them in order).
fn center_color(
    gpu: &Gpu,
    textures: Vec<Texture>,
    look: SpriteLook,
    spawn: GpuSpawn,
) -> Option<[u8; 4]> {
    let mut s = scene();
    let eye = s.frame.eye.expect("the eye");
    let camera = HdCamera::new(eye, f32::from(s.frame.fov), 200.0, SIZE.0, SIZE.1);
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    world.features.gpu_particles = ParticlePool::On;
    if !world.gpu_particles_on() {
        return None;
    }
    for t in textures {
        s.frame.textures.intern(t);
    }
    s.frame.generation += 1;
    world.upload(&s);
    s.frame.push_sprites(&SpriteFrame {
        looks: vec![look],
        spawns: vec![spawn],
        ..SpriteFrame::default()
    });
    for _ in 0..4 {
        world.draw(&s, &camera, None).expect("a frame");
    }
    let drawn = world.read_ldr().expect("the picture").rgba;
    let blank = run_blank(gpu).rgba;
    let (mut lo, mut hi) = ((u32::MAX, u32::MAX), (0, 0));
    for (i, (p, q)) in drawn.chunks(4).zip(blank.chunks(4)).enumerate() {
        if p.iter().zip(q).take(3).any(|(x, y)| x.abs_diff(*y) > 12) {
            let (x, y) = (i as u32 % SIZE.0, i as u32 / SIZE.0);
            lo = (lo.0.min(x), lo.1.min(y));
            hi = (hi.0.max(x), hi.1.max(y));
        }
    }
    let (x, y) = ((lo.0 + hi.0) / 2, (lo.1 + hi.1) / 2);
    let at = 4 * (y * SIZE.0 + x) as usize;
    Some(drawn[at..at + 4].try_into().expect("a pixel"))
}

#[test]
fn a_flipbook_shows_its_frame_and_blends_it_into_the_next_by_the_fraction_of_a_frame_when_asked() {
    let Some(gpu) = gpu() else { return };
    let frames = || {
        vec![
            solid_texture(8, [255, 0, 0, 255]),
            solid_texture(8, [0, 0, 255, 255]),
        ]
    };
    // One frame a second: at 0 ms the first, at 1000 ms the second, and at 500 ms half of each when the look blends.
    let at = |blend: bool, age_ms: i32| {
        let look = SpriteLook {
            fps: 1,
            flipbook_blend: blend,
            ..look_of(vec![0, 1])
        };
        let spawn = GpuSpawn {
            age_ms,
            size: 1.0,
            color: [1.0; 4],
            ..still(0, 0.0)
        };
        center_color(&gpu, frames(), look, spawn)
    };
    let Some(first) = at(true, 0) else {
        eprintln!("skipped: the device does not run the pool");
        return;
    };
    let second = at(true, 1000).expect("the pool");
    let half = at(true, 500).expect("the pool");
    let hard = at(false, 500).expect("the pool");
    assert!(
        first[0] > 150 && first[2] < 60,
        "the first frame: {first:?}"
    );
    assert!(
        second[2] > 150 && second[0] < 60,
        "the second frame: {second:?}"
    );
    assert!(
        half[0].abs_diff(half[2]) <= 8 && half[0] > 60 && half[2] > 60,
        "half of each: {half:?}"
    );
    // Without the blend the frame in view stays whole until the next begins.
    assert_eq!(hard, first, "a hard step");
}

/// `count` sprites `sprite_px` pixels across at 1080p, spread over the interior of the screen so none is clipped: the fill
/// governor's frame. The pool's counts, the sprites' world size, and the GPU time of its pass and draws when the device times them.
fn governed(
    gpu: &Gpu,
    sprite_px: f32,
    count: u32,
) -> Option<(torque_render_hd::SpriteCounts, Option<f64>)> {
    const HD: (u32, u32) = (1920, 1080);
    let mut s = scene();
    s.frame.size = HD;
    s.frame.eye = Some(look_at([0.0, -4.0, 2.0], [0.0, 4.0, 2.0]));
    let eye = s.frame.eye.expect("the eye");
    let camera = HdCamera::new(eye, f32::from(s.frame.fov), 200.0, HD.0, HD.1);
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    world.features.gpu_particles = ParticlePool::On;
    world.set_gpu_particle_cap(u32::MAX);
    if !world.gpu_particles_on() {
        return None;
    }
    s.frame.generation += 1;
    world.upload(&s);
    // The plane 8 units ahead: a pixel's world place, and the world size of `sprite_px` pixels there.
    let depth = 8.0;
    let (sx, sy) = (camera.proj[0], camera.proj[5]);
    let side = sprite_px * 2.0 * depth / (sy * HD.1 as f32);
    let half = sprite_px / 2.0;
    let cols = (count as f32).sqrt().ceil() as u32;
    let spawns: Vec<GpuSpawn> = (0..count)
        .map(|k| {
            let u = (k % cols) as f32 / (cols - 1).max(1) as f32;
            let v = (k / cols) as f32 / (cols - 1).max(1) as f32;
            let px = half + u * (HD.0 as f32 - sprite_px);
            let py = half + v * (HD.1 as f32 - sprite_px);
            let ndc = [px / HD.0 as f32 * 2.0 - 1.0, 1.0 - py / HD.1 as f32 * 2.0];
            GpuSpawn {
                pos: [ndc[0] * depth / sx, 4.0, 2.0 + ndc[1] * depth / sy],
                size: side,
                color: [0.6, 0.6, 0.6, 0.3],
                ..still(0, 0.0)
            }
        })
        .collect();
    s.frame.push_sprites(&SpriteFrame {
        looks: vec![look_of(Vec::new())],
        spawns,
        ..SpriteFrame::default()
    });
    for _ in 0..3 {
        world.draw(&s, &camera, None).expect("a frame");
    }
    world.finish().expect("the frames");
    let mut profile = torque_render_hd::profile::Profile::begin(&mut world);
    for _ in 0..20 {
        world.draw(&s, &camera, None).expect("a frame");
        profile.frame(&mut world);
    }
    let counts = world.gpu_particle_counts().expect("the counts")?;
    let (marks, _) = profile.gpu_medians();
    let ms = |name: &str| marks.iter().find(|m| m.0 == name).map(|m| m.1);
    let time = ms("particles").zip(ms("translucent")).map(|(a, b)| a + b);
    Some((counts, time))
}

#[test]
fn ten_thousand_big_sprites_are_held_to_the_fill_budget_within_five_percent_in_under_two_milliseconds()
 {
    let Some(gpu) = gpu() else { return };
    let Some((counts, time)) = governed(&gpu, 512.0, 10_000) else {
        eprintln!("skipped: the device does not run the pool");
        return;
    };
    assert_eq!(counts.live(), 10_000);
    let fill = counts.fill;
    // The summed area of what was handed over: 10,000 squares of 512 pixels, none clipped.
    let asked = 10_000.0 * 512.0 * 512.0;
    assert!(
        (f64::from(fill.requested) / asked - 1.0).abs() < 0.01,
        "the governor counted {} pixels of sprite, {asked} drawn",
        fill.requested
    );
    // The budget at High is eight frames of the screen.
    assert_eq!(fill.budget, 8.0 * 1920.0 * 1080.0);
    // What is drawn: the sprites at the scale the governor drew them, their area with its square.
    let drawn = f64::from(fill.requested) * f64::from(fill.size_scale).powi(2);
    assert!(
        drawn <= f64::from(fill.budget) * 1.05 && drawn >= f64::from(fill.budget) * 0.95,
        "{drawn} pixels of sprite drawn against a budget of {}",
        fill.budget
    );
    assert!(fill.alpha_scale < 1.0 && fill.alpha_scale >= 0.3);
    if let Some(ms) = time {
        assert!(ms < 2.0, "the pool's pass and draws took {ms:.3} ms");
    }
}

#[test]
fn sprites_under_the_budget_are_drawn_as_they_are() {
    let Some(gpu) = gpu() else { return };
    let Some((counts, _)) = governed(&gpu, 16.0, 2_000) else {
        eprintln!("skipped: the device does not run the pool");
        return;
    };
    assert_eq!(
        (counts.fill.size_scale, counts.fill.alpha_scale),
        (1.0, 1.0)
    );
    assert!(counts.fill.requested > 0.0 && counts.fill.requested < counts.fill.budget);
}

#[test]
fn a_blended_fast_thin_look_is_drawn_after_the_anti_aliasing_and_shows_as_the_others_do() {
    let Some(gpu) = gpu() else { return };
    let spawn = GpuSpawn {
        size: 1.0,
        color: [1.0, 0.1, 0.1, 1.0],
        ..still(0, 0.0)
    };
    let fast = SpriteLook {
        size_by_velocity: 0.02,
        ..look_of(Vec::new())
    };
    let Some(a) = covered(&gpu, &scene(), look_of(Vec::new()), spawn) else {
        eprintln!("skipped: the device does not run the pool");
        return;
    };
    // At rest a stretched look is the square (the stretch follows its motion), drawn in the over pass of the TAA frame.
    let b = covered(&gpu, &scene(), fast, spawn).expect("the pool");
    assert!(b.2 >= 100, "the late sprite covers {} pixels", b.2);
    assert!(
        a.0.abs_diff(b.0) <= 2 && a.1.abs_diff(b.1) <= 2,
        "{a:?} against {b:?}"
    );
}
