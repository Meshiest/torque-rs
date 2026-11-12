//! The GPU particle pool's motion beyond TGE's step: a look's curl-noise turbulence and its bounce off the last frame's depth, and the
//! round footprint of a sprite a few pixels across. Particles are read back from the pool (`HdWorld::gpu_particle_states`) after each
//! frame; the pictures of the small sprites are compared with an analytic disc.

use torque_render_core::Gpu;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_hd::{HdWorld, ParticlePool, PoolState};
use torque_render_scene::Blend;
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::hd::{HdCamera, HdMode, Quality, material};
use torque_render_scene::particles::{GpuSpawn, Lit, Orient, SpriteFrame, SpriteLook};

const SIZE: (u32, u32) = (320, 180);
const MS: u32 = 16;
const GRAVITY: f32 = 9.81;

fn gpu() -> Option<Gpu> {
    match Gpu::new() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipped: no GPU ({e})");
            None
        }
    }
}

/// A floor whose top is at z = 0, seen from above and behind, under the sun.
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

/// [`scene`] with a wall across the view between the eye and the particles.
fn walled() -> PlainScene {
    let mut s = scene();
    let mut wall = box_object(0, [0.0, 1.0, 2.0], [12.0, 0.2, 4.0])
        .draws
        .remove(0);
    wall.label = "wall".into();
    for v in &mut wall.vertices {
        v.material = material::KIND_PLANE << material::KIND_SHIFT;
    }
    s.frame.planes.push(wall);
    s.frame.planes_generation += 1;
    s
}

fn look(gravity: f32) -> SpriteLook {
    SpriteLook {
        label: "x".into(),
        colors: [[1.0; 4]; 4],
        sizes: [0.2; 4],
        times: [0.0, 1.0, 2.0, 2.0],
        drag: 0.0,
        gravity,
        wind: 0.0,
        blend: Blend::Alpha,
        frames: Vec::new(),
        fps: 0,
        hidden: false,
        orient: Orient::default(),
        lit: Lit::default(),
        soft_distance: 0.4,
        flipbook_blend: false,
        size_by_velocity: 0.0,
        turbulence: 0.0,
        turbulence_scale: 1.0,
        collide: false,
        bounce_elasticity: 0.5,
        bounce_friction: 0.2,
    }
}

fn bouncing(elasticity: f32, friction: f32) -> SpriteLook {
    SpriteLook {
        collide: true,
        bounce_elasticity: elasticity,
        bounce_friction: friction,
        ..look(1.0)
    }
}

fn spawn(pos: [f32; 3]) -> GpuSpawn {
    GpuSpawn {
        pos,
        age_ms: 0,
        vel: [0.0; 3],
        life_ms: 60_000,
        acc: [0.0; 3],
        spin: 0.0,
        look: 0,
        fresh: false,
        color: [1.0, 0.6, 0.2, 0.8],
        size: 0.2,
        orient_dir: [0.0, 0.0, 1.0],
    }
}

/// The pool's particles after each of `frames` frames of 16 ms with `spawns` of `look` handed over at the first. Two frames
/// of the bare scene come first: the pool bounces off the last frame's depth, which the first frame has not drawn yet.
fn fall(
    gpu: &Gpu,
    mut s: PlainScene,
    look: &SpriteLook,
    spawns: &[GpuSpawn],
    frames: usize,
) -> Option<Vec<Vec<PoolState>>> {
    let eye = s.frame.eye.expect("the eye");
    let camera = HdCamera::new(eye, f32::from(s.frame.fov), 200.0, SIZE.0, SIZE.1);
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    world.features.gpu_particles = ParticlePool::On;
    if !world.gpu_particles_on() {
        eprintln!("skipped: the device does not run the pool");
        return None;
    }
    s.frame.generation += 1;
    world.upload(&s);
    let mut states = Vec::new();
    for k in 0..frames + 2 {
        s.frame.push_sprites(&SpriteFrame {
            looks: vec![look.clone()],
            spawns: if k == 2 { spawns.to_vec() } else { Vec::new() },
            steps: vec![MS],
            ..SpriteFrame::default()
        });
        world.draw(&s, &camera, None).expect("a frame");
        if k >= 2 {
            states.push(
                world
                    .gpu_particle_states()
                    .expect("the particles")
                    .expect("a pool"),
            );
        }
    }
    Some(states)
}

fn speed(p: &PoolState) -> f32 {
    p.vel.iter().map(|v| v * v).sum::<f32>().sqrt()
}

#[test]
fn a_particle_falling_onto_a_floor_stops_above_it_and_bounces_by_the_restitution() {
    let Some(gpu) = gpu() else { return };
    let elasticity = 0.5;
    let Some(run) = fall(
        &gpu,
        scene(),
        &bouncing(elasticity, 0.2),
        &[spawn([0.0, 4.0, 1.5])],
        150,
    ) else {
        return;
    };
    let track: Vec<PoolState> = run.iter().map(|f| f[0]).collect();
    let lowest = track.iter().map(|p| p.pos[2]).fold(f32::MAX, f32::min);
    assert!(lowest > -0.01, "the particle sank to {lowest}");
    // The first bounce: the speed down before the step that met the floor, less a step's gravity, kept by the restitution.
    let hit = track
        .windows(2)
        .position(|w| w[0].vel[2] < 0.0 && w[1].vel[2] > 0.0)
        .expect("the particle bounced");
    let (before, after) = (track[hit].vel[2], track[hit + 1].vel[2]);
    let into = -before + GRAVITY * MS as f32 / 1000.0;
    let kept = after / into;
    assert!(
        (kept - elasticity).abs() < 0.05,
        "kept {kept} of the speed into the floor ({into} then {after}), asked {elasticity}"
    );
    // It comes to rest on the floor.
    let last = track.last().expect("frames");
    assert!(
        (0.0..0.1).contains(&last.pos[2]) && speed(last) < 0.2,
        "ends at {:?} moving {:?}",
        last.pos,
        last.vel
    );
}

#[test]
fn a_look_that_does_not_collide_falls_through_the_floor() {
    let Some(gpu) = gpu() else { return };
    let Some(run) = fall(&gpu, scene(), &look(1.0), &[spawn([0.0, 4.0, 1.5])], 100) else {
        return;
    };
    assert!(run.last().expect("frames")[0].pos[2] < -3.0);
}

#[test]
fn a_particle_off_the_screen_or_behind_geometry_keeps_flying() {
    let Some(gpu) = gpu() else { return };
    let look = bouncing(0.5, 0.2);
    // The floor extends far past the view's edge, where the depth shows nothing.
    let Some(off) = fall(&gpu, scene(), &look, &[spawn([18.0, 4.0, 1.5])], 100) else {
        return;
    };
    // Behind a wall the depth shows the wall, farther than a step's reach in front of the particle.
    let behind = fall(&gpu, walled(), &look, &[spawn([0.0, 4.0, 1.5])], 100).expect("the pool");
    for (name, run) in [("off the screen", off), ("behind the wall", behind)] {
        let z = run.last().expect("frames")[0].pos[2];
        assert!(z < -3.0, "{name}: the particle stopped at {z}");
    }
}

/// `n` particles at rest on a line, in a look with no gravity.
fn cluster(n: usize) -> Vec<GpuSpawn> {
    (0..n)
        .map(|k| {
            spawn([
                k as f32 * 0.1 - 3.0,
                4.0 + (k % 7) as f32 * 0.3,
                1.0 + (k % 5) as f32 * 0.2,
            ])
        })
        .collect()
}

fn displacements(run: &[Vec<PoolState>], start: &[GpuSpawn]) -> Vec<f32> {
    run.last()
        .expect("frames")
        .iter()
        .zip(start)
        .map(|(p, s)| {
            (0..3)
                .map(|k| (p.pos[k] - s.pos[k]).powi(2))
                .sum::<f32>()
                .sqrt()
        })
        .collect()
}

#[test]
fn turbulence_off_moves_nothing_and_on_moves_particles_by_a_bounded_amount_the_same_every_run() {
    let Some(gpu) = gpu() else { return };
    let start = cluster(64);
    let frames = 62;
    let calm = |scale: f32| SpriteLook {
        turbulence: 0.0,
        turbulence_scale: scale,
        ..look(0.0)
    };
    let Some(a) = fall(&gpu, scene(), &calm(1.0), &start, frames) else {
        return;
    };
    let b = fall(&gpu, scene(), &calm(7.5), &start, frames).expect("the pool");
    // No turbulence: the scale means nothing and a particle with no force stays where it was put.
    assert_eq!(a, b);
    assert!(
        displacements(&a, &start).iter().all(|&d| d == 0.0),
        "a particle moved with no force on it"
    );
    let strength = 2.0;
    let swirling = SpriteLook {
        turbulence: strength,
        ..look(0.0)
    };
    let c = fall(&gpu, scene(), &swirling, &start, frames).expect("the pool");
    let d = fall(&gpu, scene(), &swirling, &start, frames).expect("the pool");
    assert_eq!(c, d, "two runs differ");
    let moved = displacements(&c, &start);
    let seconds = frames as f32 * MS as f32 / 1000.0;
    // The noise is a unit-ish field: an acceleration of at most a few times the strength, for the time it ran.
    let bound = 0.5 * strength * 20.0 * seconds * seconds;
    let mean = moved.iter().sum::<f32>() / moved.len() as f32;
    assert!(
        moved.iter().all(|d| d.is_finite() && *d < bound),
        "a particle moved {:?}, past {bound}",
        moved.iter().copied().fold(0.0, f32::max)
    );
    assert!(mean > 0.05, "the particles moved {mean} on average");
    // Different places swirl differently.
    let spread = moved.iter().map(|d| (d - mean).abs()).sum::<f32>() / moved.len() as f32;
    assert!(spread > 0.01, "every particle moved {mean}");
}

/// The picture of one black sprite `pixels` across at the middle of a bare-sky view (None: no GPU, or one that does not run the
/// pool), and of the sky alone: the sprite's pixels as their change of brightness from the sky's.
fn dot(gpu: &Gpu, pixels: f32) -> Option<Vec<f32>> {
    let mut s = scene();
    s.mode = Some(HdMode::new(Quality::Low));
    s.frame.planes.clear();
    s.frame.planes_generation += 1;
    s.frame.eye = Some(look_at([0.0, -4.0, 2.0], [0.0, 4.0, 3.0]));
    let eye = s.frame.eye.expect("the eye");
    let camera = HdCamera::new(eye, f32::from(s.frame.fov), 200.0, SIZE.0, SIZE.1);
    // On the view's axis ten units out, where a unit is `proj` x half the height pixels.
    let ahead = [0.0, 8.0, 1.0];
    let length = 65.0f32.sqrt();
    let at = [
        0.0,
        -4.0 + 10.0 * ahead[1] / length,
        2.0 + 10.0 * ahead[2] / length,
    ];
    let per_unit = camera.proj[5] * SIZE.1 as f32 / 2.0 / 10.0;
    let draw = |sprites: Option<SpriteFrame>| {
        let mut s = s.clone();
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
        looks: vec![look(0.0)],
        spawns: vec![GpuSpawn {
            size: pixels / per_unit,
            color: [0.0, 0.0, 0.0, 1.0],
            ..spawn(at)
        }],
        ..SpriteFrame::default()
    }))?;
    let blank = draw(None)?;
    let brightness = |p: &[u8]| p.iter().take(3).map(|&c| f32::from(c)).sum::<f32>() / 765.0;
    Some(
        drawn
            .chunks(4)
            .zip(blank.chunks(4))
            .map(|(p, q)| (brightness(p) - brightness(q)).abs())
            .collect(),
    )
}

/// The pixel at (`x`, `y`) of `dot`'s picture.
fn pixel(picture: &[f32], x: u32, y: u32) -> f32 {
    picture[(y * SIZE.0 + x) as usize]
}

#[test]
fn a_sprite_three_pixels_across_is_a_round_dot_not_a_square() {
    let Some(gpu) = gpu() else { return };
    let Some(picture) = dot(&gpu, 3.0) else {
        eprintln!("skipped: the device does not run the pool");
        return;
    };
    // The view's middle is the corner of four pixels: the dot is symmetric about it. The pixels by distance from the corner.
    let (cx, cy) = (SIZE.0 / 2, SIZE.1 / 2);
    let ring = |dx: i32, dy: i32| {
        let at = |sx: i32, sy: i32| {
            let x = (cx as i32 + if sx > 0 { dx } else { -dx - 1 }) as u32;
            let y = (cy as i32 + if sy > 0 { dy } else { -dy - 1 }) as u32;
            pixel(&picture, x, y)
        };
        [at(1, 1), at(-1, 1), at(1, -1), at(-1, -1)]
    };
    let inner = ring(0, 0);
    let edge = [ring(1, 0), ring(0, 1)].concat();
    let corner = ring(1, 1);
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
    let (inner, edge, corner) = (mean(&inner), mean(&edge), mean(&corner));
    // The disc: a pixel's share of a round dot of its footprint falls from the middle outward, and no pixel further out shows.
    assert!(inner > 0.1, "the dot's middle changes {inner}");
    assert!(
        edge < 0.9 * inner && edge > 0.5 * inner,
        "{inner:.3} in the middle, {edge:.3} at the edges"
    );
    assert!(
        corner < 0.6 * inner && corner < 0.75 * edge,
        "{inner:.3} in the middle, {edge:.3} at the edges, {corner:.3} at the corners"
    );
    let outside = (0..SIZE.0)
        .flat_map(|x| (0..SIZE.1).map(move |y| (x, y)))
        .filter(|&(x, y)| x.abs_diff(cx) > 3 && y.abs_diff(cy) > 3)
        .map(|(x, y)| pixel(&picture, x, y))
        .fold(0.0, f32::max);
    assert!(outside < 0.02, "the dot reaches {outside} far outside");
    // The four sides of the dot agree.
    for r in [ring(0, 0), ring(1, 0), ring(0, 1)] {
        let (lo, hi) = r
            .iter()
            .fold((f32::MAX, 0.0f32), |(l, h), &v| (l.min(v), h.max(v)));
        assert!(
            hi - lo < 0.15 * inner,
            "a ring differs from side to side: {r:?}"
        );
    }
}

#[test]
fn a_sprite_a_dozen_pixels_across_is_left_square() {
    let Some(gpu) = gpu() else { return };
    let Some(picture) = dot(&gpu, 12.0) else {
        eprintln!("skipped: the device does not run the pool");
        return;
    };
    let (cx, cy) = (SIZE.0 / 2, SIZE.1 / 2);
    // The square's corner pixels are nearly as bright as its middle (a round dot's would be dark).
    let middle = pixel(&picture, cx, cy);
    let corner = pixel(&picture, cx + 4, cy + 4);
    assert!(
        middle > 0.1 && corner > 0.6 * middle,
        "{middle:.3} in the middle, {corner:.3} at a corner"
    );
}
