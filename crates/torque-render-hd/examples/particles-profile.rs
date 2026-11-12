//! The particles' cost a frame, through the GPU particle pool or the CPU path: the bench scene (a floor, 64 figures) with
//! `--count` particles alive from 100 emitters over the floor (lit smoke blended by its alpha and added sparks, a soft round
//! texture, about 8 pixels at 1080p: 100,000 of them are 100 dense clouds; `--scale` sizes them), stepped 16 ms a frame. After the particles fill up, `--frames` frames are drawn
//! twice: with the particles, then with none (the same world and scene), so the particles' GPU time is the pool's pass plus what
//! the translucent passes take more with them.
//!
//! Printed: the GPU pass medians of both runs (`Profile`), the particles' GPU time (the `particles` mark plus the translucent
//! marks' difference), the scene side's CPU a frame (the emitters' emission and steps, then the hand-over or the quads, and the
//! scene's take of them) and the draw's `begin: particles` mark, and the particles alive.
//!
//! `cargo run -p torque-render-hd --release --example particles-profile -- [--count N] [--quality low|medium|high|ultra]
//! [--size WxH] [--frames N] [--scale S] [--cpu]`. A timing: run it under the GPU bench lock.

#[path = "../benches/common/mod.rs"]
mod common;

use torque_render_core::Gpu;
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_hd::profile::Profile;
use torque_render_scene::hd::particle_light::ParticleLight;
use torque_render_scene::hd::{HdCamera, HdMode, Quality};
use torque_render_scene::particles::{Emitter, EmitterData, ParticleData, Rng, SpriteFrame};
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Assets, Draw};

const EMITTERS: u32 = 100;
const LIFE_MS: i32 = 2000;
const MS: u32 = 16;
/// The GPU marks the particles' draws fall in (the weighted blend and the late sprites are in the translucent one).
const TRANSLUCENT: &str = "translucent";

struct Args {
    count: u32,
    quality: Quality,
    size: (u32, u32),
    frames: usize,
    cpu: bool,
    /// The sprites' size times this.
    scale: f32,
}

fn args() -> Result<Args, String> {
    let mut a = Args {
        count: 100_000,
        quality: Quality::High,
        size: (1920, 1080),
        frames: 200,
        cpu: false,
        scale: 1.0,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--count" => a.count = value()?.parse().map_err(|e| format!("--count: {e}"))?,
            "--frames" => a.frames = value()?.parse().map_err(|e| format!("--frames: {e}"))?,
            "--quality" => {
                let q = value()?;
                a.quality = Quality::ALL
                    .into_iter()
                    .find(|x| x.name() == q)
                    .ok_or(format!("no quality {q}"))?;
            }
            "--size" => {
                let v = value()?;
                let (w, h) = v.split_once('x').ok_or(format!("--size {v}: WxH"))?;
                a.size = (
                    w.parse().map_err(|e| format!("--size: {e}"))?,
                    h.parse().map_err(|e| format!("--size: {e}"))?,
                );
            }
            "--cpu" => a.cpu = true,
            "--scale" => a.scale = value()?.parse().map_err(|e| format!("--scale: {e}"))?,
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    Ok(a)
}

/// A soft round puff, white with its alpha falling to the edge, as a PNG for any `puff` path.
struct Puff(Vec<u8>);

impl Puff {
    fn new() -> Puff {
        const SIDE: u32 = 32;
        let mut texels = Vec::with_capacity((SIDE * SIDE * 4) as usize);
        for y in 0..SIDE {
            for x in 0..SIDE {
                let d = ((x as f32 + 0.5) / SIDE as f32 - 0.5)
                    .hypot((y as f32 + 0.5) / SIDE as f32 - 0.5);
                let a = (1.0 - d * 2.0).clamp(0.0, 1.0);
                texels.extend_from_slice(&[255, 255, 255, (a * a * 255.0) as u8]);
            }
        }
        let mut png = Vec::new();
        let mut enc = png::Encoder::new(&mut png, SIDE, SIDE);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("a png header");
        w.write_image_data(&texels).expect("the png's texels");
        drop(w);
        Puff(png)
    }
}

impl Assets for Puff {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        path.ends_with("puff.png").then(|| self.0.clone())
    }
}

/// Emitter `k` of [`EMITTERS`]: even ones lit smoke blended by its alpha, odd ones added sparks, each keeping `count /
/// EMITTERS` particles alive.
fn emitter(k: u32, count: u32, scale: f32) -> Emitter {
    let smoke = k.is_multiple_of(2);
    let period = ((LIFE_MS as u32 * EMITTERS) / count.max(1)).max(1) as i32;
    let data = EmitterData {
        period_ms: period,
        period_variance_ms: 0,
        velocity: if smoke { 0.8 } else { 2.5 },
        velocity_variance: 0.4,
        offset: 0.0,
        theta_min: 0.0,
        theta_max: 60.0,
        phi_ref_vel: 0.0,
        phi_variance: 360.0,
        override_advance: false,
        lifetime_ms: 0,
        use_emitter_sizes: false,
        use_emitter_colors: false,
        particles: vec![1],
        extras: torque_render_scene::particles::EmitterExtras::default(),
    };
    let p = ParticleData {
        drag: 0.4,
        wind: 0.0,
        gravity: if smoke { -0.05 } else { 0.3 },
        inherited_vel: 0.0,
        constant_accel: 0.0,
        lifetime_ms: LIFE_MS,
        lifetime_variance_ms: 0,
        spin_speed: 20.0,
        spin_min: -20.0,
        spin_max: 20.0,
        inv_alpha: smoke,
        colors: if smoke {
            [
                [0.6, 0.6, 0.6, 0.0],
                [0.6, 0.6, 0.6, 0.6],
                [0.5, 0.5, 0.5, 0.3],
                [0.5, 0.5, 0.5, 0.0],
            ]
        } else {
            [
                [1.0, 0.8, 0.4, 1.0],
                [1.0, 0.6, 0.2, 0.8],
                [0.8, 0.3, 0.1, 0.4],
                [0.5, 0.1, 0.0, 0.0],
            ]
        },
        sizes: [0.06, 0.08, 0.09, 0.1].map(|x| x * scale),
        times: [0.0, 0.2, 0.7, 1.0],
        textures: vec!["fx/puff".to_string()],
        animate: false,
        fps: 0,
        extras: torque_render_scene::particles::ParticleExtras::default(),
    };
    Emitter::of_data(data, vec![p]).expect("an emitter")
}

/// The emitters, each with its own count of live particles (the CPU path's cap is for all of a client's emitters: here each
/// keeps its share under it, so the CPU path can be timed at the same count).
struct Sources {
    emitters: Vec<(Emitter, usize, [f32; 3])>,
    rng: Rng,
}

impl Sources {
    fn new(count: u32, scale: f32) -> Sources {
        let emitters = (0..EMITTERS)
            .map(|k| {
                let at = [
                    (k % 10) as f32 * 1.6 - 7.2,
                    (k / 10) as f32 * 1.2 - 1.0,
                    0.3,
                ];
                (emitter(k, count, scale), 0, at)
            })
            .collect();
        Sources {
            emitters,
            rng: Rng::new(0x5eed),
        }
    }

    /// A frame: every emitter stepped and run, its particles handed over (`pool`) or drawn into the scene.
    fn frame(
        &mut self,
        s: &mut PlainScene,
        assets: &dyn Assets,
        pool: bool,
        right: [f32; 3],
        up: [f32; 3],
    ) {
        let mut set = TextureSet::new(assets);
        let mut sprites = SpriteFrame {
            steps: vec![MS],
            ..SpriteFrame::default()
        };
        let mut draws: Vec<(f32, Draw)> = Vec::new();
        for (e, live, at) in &mut self.emitters {
            e.advance(MS, live);
            e.emit_path(*at, [0.0, 0.0, 1.0], [0.0; 3], MS, &mut self.rng, live);
            if pool {
                e.hand_over(&mut sprites, &mut set, "profile", live);
            } else if let Some(d) = e.draw(right, up, &mut set, "profile") {
                draws.push((10.0, d));
            }
        }
        s.frame
            .set_particles_decals_and_sprites(&draws, &[], pool.then_some(&sprites), set);
    }

    fn live(&self) -> usize {
        self.emitters
            .iter()
            .map(|(e, _, _)| e.particles.len())
            .sum()
    }
}

fn mark(rows: &[(&'static str, f64)], name: &str) -> f64 {
    rows.iter().find(|(n, _)| *n == name).map_or(0.0, |r| r.1)
}

fn main() -> Result<(), String> {
    let a = args()?;
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let assets = Puff::new();
    let mut s = common::scene(Some(1), false);
    s.mode = Some(HdMode::new(a.quality));
    s.frame.particle_light = Some(ParticleLight::BLENDED);
    s.frame.size = a.size;
    let eye = s.frame.eye.expect("the scene's eye");
    let camera = HdCamera::new(
        eye,
        f32::from(s.frame.fov),
        s.frame.visible_distance,
        a.size.0,
        a.size.1,
    );
    let right = [camera.view[0], camera.view[4], camera.view[8]];
    let up = [camera.view[1], camera.view[5], camera.view[9]];
    let mut world: HdWorld<NoExtension> = HdWorld::new(&gpu);
    world.features.gpu_particles = !a.cpu;
    // The quality's whole pool (by default it holds no more than the CPU path's cap).
    world.set_gpu_particle_cap(u32::MAX);
    let pool = world.gpu_particles_on();
    if !a.cpu && !pool {
        return Err("this device does not run the GPU particle pool".into());
    }
    let mut sources = Sources::new(a.count, a.scale);
    s.frame.generation += 1;
    world.upload(&s);
    let draw = |world: &mut HdWorld<NoExtension>, s: &mut PlainScene| -> Result<(), String> {
        s.frame.generation += 1;
        s.frame.control_generation += 1;
        world.upload(s);
        world.draw(s, &camera, None)
    };
    // The particles fill up (a life and a half), then the frames timed.
    let fill = (LIFE_MS as u32 * 3 / 2).div_ceil(MS);
    for _ in 0..fill {
        sources.frame(&mut s, &assets, pool, right, up);
        draw(&mut world, &mut s)?;
    }
    world.finish()?;
    let mut with = Profile::begin(&mut world);
    let mut scene_ms = 0.0;
    for _ in 0..a.frames {
        let t = web_time::Instant::now();
        sources.frame(&mut s, &assets, pool, right, up);
        scene_ms += t.elapsed().as_secs_f64() * 1000.0;
        draw(&mut world, &mut s)?;
        with.frame(&mut world);
    }
    let (live, dropped) = if pool {
        world
            .gpu_particle_counts()?
            .map_or((0, 0), |c| (c.live() as usize, c.dropped))
    } else {
        (sources.live(), 0)
    };
    // The same world with no particle: the frame without them.
    s.frame.set_particles_decals_and_sprites(
        &[],
        &[],
        pool.then_some(&SpriteFrame {
            clear: true,
            ..SpriteFrame::default()
        }),
        TextureSet::new(&assets),
    );
    s.frame.particles.clear();
    s.frame.particles_generation += 1;
    for _ in 0..30 {
        draw(&mut world, &mut s)?;
    }
    let mut without = Profile::begin(&mut world);
    for _ in 0..a.frames {
        draw(&mut world, &mut s)?;
        without.frame(&mut world);
    }
    let (w, h) = a.size;
    let way = if pool { "the GPU pool" } else { "the CPU path" };
    println!(
        "particles-profile: {live} particles alive through {way} ({dropped} spawns dropped), {} at {w}x{h}, {} frames",
        a.quality.name(),
        a.frames
    );
    println!("with the particles:\n{}", with.report(0.01));
    println!("without them:\n{}", without.report(0.01));
    let (g1, _) = with.gpu_medians();
    let (g0, _) = without.gpu_medians();
    let sim = mark(&g1, "particles");
    let drawn = mark(&g1, TRANSLUCENT) - mark(&g0, TRANSLUCENT);
    let c1 = with.cpu_means();
    let begin = mark(&c1, "begin: particles") - mark(&without.cpu_means(), "begin: particles");
    println!(
        "particles gpu: {:.3} ms (pass {sim:.3}, draws {drawn:.3}); cpu: scene {:.3} ms a frame, `begin: particles` {:.3} ms ({begin:+.3} over none)",
        sim + drawn,
        scene_ms / a.frames as f64,
        mark(&c1, "begin: particles"),
    );
    Ok(())
}
