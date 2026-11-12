use super::*;
use crate::hd::kit::box_object;
use crate::particles::{EmitterData, EmitterExtras, ParticleData, ParticleExtras};

fn sand() -> Emitter {
    let data = EmitterData {
        period_ms: 10,
        period_variance_ms: 0,
        velocity: 0.5,
        velocity_variance: 0.1,
        offset: 0.0,
        theta_min: 0.0,
        theta_max: 30.0,
        phi_ref_vel: 0.0,
        phi_variance: 360.0,
        override_advance: false,
        lifetime_ms: 0,
        use_emitter_sizes: false,
        use_emitter_colors: false,
        particles: vec![1],
        extras: EmitterExtras::default(),
    };
    let p = ParticleData {
        drag: 0.2,
        wind: 0.0,
        gravity: 1.0,
        inherited_vel: 0.0,
        constant_accel: 0.0,
        lifetime_ms: 60_000,
        lifetime_variance_ms: 0,
        spin_speed: 0.0,
        spin_min: 0.0,
        spin_max: 0.0,
        inv_alpha: true,
        colors: [[1.0, 0.9, 0.7, 1.0]; 4],
        sizes: [0.05; 4],
        times: [0.0, 1.0, 1.0, 1.0],
        textures: Vec::new(),
        animate: false,
        fps: 0,
        extras: ParticleExtras::default(),
    };
    Emitter::of_data(data, vec![p]).expect("an emitter")
}

fn look(density: f32) -> Look {
    Look {
        duration_ms: 1000,
        emitter: Some(7),
        density,
        ..Look::default()
    }
}

/// Runs `frames` frames of `dt` over `objects`: the particles each frame's fronts made.
fn run(d: &mut Dissolves, objects: &[DynamicObject], frames: usize, dt: f32) -> Vec<usize> {
    (0..frames)
        .map(|_| {
            d.frame(objects, &[], dt);
            d.spawned
        })
        .collect()
}

/// The sand emitter of datablock 7.
fn make(db: u32) -> Option<Emitter> {
    (db == 7).then(sand)
}

#[test]
fn the_drawn_share_of_the_samples_is_one_less_the_eaten_share() {
    let o = box_object(1, [0.0, 0.0, 1.0], [0.5, 0.5, 1.0]);
    let s = sample_surface(&o, &[], &mut TextureMeans::default(), &look(400.0));
    // 2 * (1 + 2 + 2) square units of a 1 x 1 x 2 box.
    assert!((s.area - 10.0).abs() < 1e-3, "{}", s.area);
    assert_eq!(s.len(), 4000);
    for eaten in [0.1f32, 0.25, 0.5, 0.75, 0.9] {
        let t = s.quantile(eaten);
        let drawn = s.samples.iter().filter(|p| p.field >= t).count() as f32 / s.len() as f32;
        assert!((drawn - (1.0 - eaten)).abs() < 0.01, "{eaten}: {drawn}");
    }
    assert_eq!(s.quantile(0.0), WHOLE);
    assert_eq!(s.quantile(1.0), GONE);
}

#[test]
fn the_fronts_particles_track_the_area_it_swept_and_its_caps() {
    // A box of 10 square units at 50 a unit, eaten over 1 s in frames of 50 ms: 25 particles a frame.
    let small = box_object(1, [0.0, 0.0, 1.0], [0.5, 0.5, 1.0]);
    let mut d = Dissolves::default();
    d.sync(1, Some((1, look(50.0), 0)), make);
    let made = run(&mut d, std::slice::from_ref(&small), 22, 0.05);
    // The first frame finds the surface and emits nothing; the next nineteen each a twentieth of the samples, then none.
    assert_eq!(made[0], 0);
    let total: usize = made.iter().sum();
    assert!(total.abs_diff(475) <= 2, "{made:?}");
    assert!(
        made[1..20].iter().all(|&n| (24..=26).contains(&n)),
        "{made:?}"
    );
    assert!(made[20..].iter().all(|&n| n == 0), "{made:?}");
    // Twice the area at the same density: twice the particles a frame.
    let big = box_object(2, [0.0, 0.0, 2.25], [0.5, 0.5, 2.25]);
    let mut d = Dissolves::default();
    d.sync(2, Some((1, look(50.0), 0)), make);
    let s2 = sample_surface(&big, &[], &mut TextureMeans::default(), &look(50.0));
    let made2 = run(&mut d, &[big], 5, 0.05);
    let expect = s2.len() / 20;
    assert!(
        made2[1..].iter().all(|&n| n.abs_diff(expect) <= 1),
        "{made2:?} vs {expect}"
    );
    assert_eq!(expect, 50);
    // Denser than the samples' cap: each sample stands for several grains, so the count still tracks the area swept.
    let s = sample_surface(&small, &[], &mut TextureMeans::default(), &look(4000.0));
    assert_eq!((s.len(), s.grains), (MAX_SAMPLES, 3));
    let mut d = Dissolves::default();
    d.sync(1, Some((1, look(4000.0), 0)), make);
    let made = run(&mut d, std::slice::from_ref(&small), 4, 0.02);
    assert!(
        made[1..].iter().all(|&n| n.abs_diff(328 * 3) <= 6),
        "{made:?}"
    );
    // A dense surface swept fast meets the object's cap, and many objects the frame's.
    let objects: Vec<DynamicObject> = (0..8)
        .map(|g| box_object(10 + g, [g as f32 * 3.0, 0.0, 1.0], [0.5, 0.5, 1.0]))
        .collect();
    let mut d = Dissolves::default();
    for o in &objects {
        d.sync(o.ghost, Some((1, look(4000.0), 0)), make);
    }
    let made = run(&mut d, &objects, 3, 0.1);
    assert_eq!(made[1], MAX_SPAWNS_PER_FRAME);
    const { assert!(MAX_SPAWNS_PER_FRAME < 8 * MAX_SPAWNS_PER_OBJECT) };
}

/// A server that starts a dissolve on every ghost every tick costs the client a bounded amount a frame: at most
/// `MAX_SAMPLINGS_PER_FRAME` surfaces sampled (the others drawn whole until their turn), a ghost restarted at most once every
/// `MIN_RESTART_MS`, and a restart keeps its samples while the shape and the noise are the same.
#[test]
fn churning_dissolves_sample_a_bounded_number_of_surfaces_a_frame() {
    let objects: Vec<DynamicObject> = (0..MAX_OBJECTS as u32)
        .map(|g| box_object(g + 1, [g as f32 * 3.0, 0.0, 1.0], [0.5, 0.5, 1.0]))
        .collect();
    let mut d = Dissolves::default();
    let mut sampled = 0;
    let mut whole = 0;
    for frame in 0..60u32 {
        for o in &objects {
            d.sync(o.ghost, Some((frame, look(4000.0), 0)), make);
        }
        let out = d.frame(&objects, &[], 1.0 / 60.0);
        assert!(
            d.sampled <= MAX_SAMPLINGS_PER_FRAME,
            "frame {frame}: {}",
            d.sampled
        );
        sampled += d.sampled;
        whole += out.iter().filter(|h| h.threshold == WHOLE).count();
    }
    // Each surface once (the restarts keep them), the rest drawn whole while they waited.
    assert_eq!(sampled, MAX_OBJECTS);
    assert!(whole > 0);
    // A second of restarts: about four a ghost.
    let restarts = (0..60u32)
        .filter(|&f| {
            d.sync(1, Some((1000 + f, look(4000.0), 0)), make);
            d.frame(&objects[..1], &[], 1.0 / 60.0);
            d.items[&1].serial == 1000 + f
        })
        .count();
    assert!((3..=5).contains(&restarts), "{restarts}");
}

#[test]
fn a_dissolve_whose_emitter_arrives_late_makes_its_particles_then() {
    let o = box_object(1, [0.0, 0.0, 1.0], [0.5, 0.5, 1.0]);
    let mut d = Dissolves::default();
    d.sync(1, Some((1, look(50.0), 0)), |_| None);
    assert_eq!(run(&mut d, std::slice::from_ref(&o), 3, 0.05), [0, 0, 0]);
    // The datablock arrives: the next sync makes the emitter.
    d.sync(1, Some((1, look(50.0), 0)), make);
    assert!(
        run(&mut d, std::slice::from_ref(&o), 2, 0.05)
            .iter()
            .all(|&n| n > 20)
    );
}

#[test]
fn a_materialize_runs_the_same_front_backward_and_a_new_serial_starts_again() {
    let o = box_object(1, [0.0, 0.0, 1.0], [0.5, 0.5, 1.0]);
    let mut d = Dissolves::default();
    let back = Look {
        direction: Direction::In,
        ..look(50.0)
    };
    d.sync(1, Some((4, back, 250)), make);
    assert_eq!(d.eaten(1), Some(0.75));
    let made = run(&mut d, std::slice::from_ref(&o), 4, 0.05);
    assert!(
        made[1..].iter().all(|&n| (24..=26).contains(&n)),
        "{made:?}"
    );
    // The same serial again keeps the clock; another starts it over (once the restart's spacing has passed).
    d.sync(1, Some((4, back, 0)), make);
    assert!((d.eaten(1).unwrap() - 0.55).abs() < 1e-4);
    d.sync(1, Some((5, look(50.0), 500)), make);
    assert!((d.eaten(1).unwrap() - 0.55).abs() < 1e-4, "too soon");
    run(&mut d, std::slice::from_ref(&o), 1, 0.05);
    d.sync(1, Some((5, look(50.0), 500)), make);
    assert_eq!(d.eaten(1), Some(0.5));
    d.sync(1, None, make);
    assert_eq!(d.eaten(1), None);
}

#[test]
fn hostile_looks_are_held_to_their_ranges() {
    let l = Look {
        duration_ms: u32::MAX,
        edge_color: [f32::NAN, 9.0, -1.0],
        glow: f32::INFINITY,
        edge_share: 7.0,
        cell: 0.0,
        sweep: -9.0,
        density: 1e12,
        ..Look::default()
    }
    .held();
    assert_eq!(l.duration_ms, MAX_DURATION_MS);
    assert_eq!(l.edge_color, [0.0, 1.0, 0.0]);
    assert_eq!(l.glow, Look::default().glow);
    assert_eq!((l.edge_share, l.cell, l.sweep), (1.0, MIN_CELL, -1.0));
    assert_eq!(l.density, MAX_DENSITY);
    // A huge object at the largest density takes the samples' cap.
    let o = box_object(1, [0.0; 3], [100.0, 100.0, 100.0]);
    assert_eq!(
        sample_surface(&o, &[], &mut TextureMeans::default(), &l).len(),
        MAX_SAMPLES
    );
}

#[test]
fn a_top_faces_grains_spill_out_from_the_bodys_axis() {
    let up = glam::Vec3::Z;
    // On the top face off to +x: thrown up and out along +x, never back toward the axis.
    let a = spill_axis(up, glam::Vec3::new(0.4, 0.0, 2.0));
    assert!(a.x > 0.5 && a.z > 0.5 && a.y.abs() < 1e-6, "{a}");
    // A side face keeps its outward normal; a point on the axis keeps the normal alone.
    let side = spill_axis(glam::Vec3::X, glam::Vec3::new(0.5, 0.0, 1.0));
    assert!((side - glam::Vec3::X).length() < 1e-6, "{side}");
    assert_eq!(spill_axis(up, glam::Vec3::new(0.0, 0.0, 2.0)), up);
}
