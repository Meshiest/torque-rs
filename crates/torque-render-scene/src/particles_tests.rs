use super::*;
use crate::Assets;
use crate::world::tests::datablock;

fn data() -> (EmitterData, ParticleData) {
    (
        EmitterData {
            period_ms: 10,
            period_variance_ms: 0,
            velocity: 2.0,
            velocity_variance: 0.0,
            offset: 0.0,
            theta_min: 0.0,
            theta_max: 0.0,
            phi_ref_vel: 0.0,
            phi_variance: 0.0,
            override_advance: false,
            lifetime_ms: 0,
            use_emitter_sizes: false,
            use_emitter_colors: false,
            particles: vec![1],
        },
        ParticleData {
            drag: 0.0,
            wind: 0.0,
            gravity: 0.0,
            inherited_vel: 0.0,
            constant_accel: 0.0,
            lifetime_ms: 200,
            lifetime_variance_ms: 0,
            spin_speed: 0.0,
            spin_min: 0.0,
            spin_max: 0.0,
            inv_alpha: true,
            colors: [
                [1.0, 0.0, 0.0, 1.0],
                [1.0, 0.0, 0.0, 0.0],
                [1.0; 4],
                [1.0; 4],
            ],
            sizes: [0.2, 0.4, 1.0, 1.0],
            times: [0.0, 1.0, 2.0, 2.0],
            textures: Vec::new(),
            animate: false,
            fps: 0,
        },
    )
}

fn emitter() -> Emitter {
    let (d, p) = data();
    Emitter {
        data: d,
        particle_data: vec![p],
        particles: Vec::new(),
        next_ms: 0,
        clock_ms: 0,
        last: None,
        near_z_cross: [0.0, 1.0, 0.0],
        colors: None,
        colors_inv_alpha: false,
        dead: false,
    }
}

#[test]
fn a_path_emits_a_particle_a_period_and_they_fly_along_the_axis_and_die() {
    let mut e = emitter();
    let mut rng = Rng::new(7);
    let mut live = 0;
    e.emit_path(
        [0.0; 3],
        [0.0, 1.0, 0.0],
        [0.0; 3],
        100,
        &mut rng,
        &mut live,
    );
    assert_eq!(e.particles.len(), 10);
    // The oldest (made first, at 10 ms) has run 90 ms at 2 m/s along y.
    let oldest = e.particles.first().unwrap();
    assert!((oldest.pos[1] - 0.18).abs() < 1e-4, "{:?}", oldest.pos);
    assert!(
        (oldest.size - (0.2 + 0.2 * 0.45)).abs() < 1e-4,
        "{}",
        oldest.size
    );
    e.advance(150, &mut live);
    assert!(e.particles.len() < 10);
    e.advance(500, &mut live);
    assert!(e.particles.is_empty());
    assert_eq!(live, 0);
}

#[test]
fn the_time_to_the_next_particle_carries_across_calls() {
    let mut e = emitter();
    e.data.period_ms = 30;
    let mut rng = Rng::new(1);
    let mut live = 0;
    e.emit_path([0.0; 3], [0.0, 0.0, 1.0], [0.0; 3], 16, &mut rng, &mut live);
    e.emit_path([0.0; 3], [0.0, 0.0, 1.0], [0.0; 3], 16, &mut rng, &mut live);
    assert_eq!(e.particles.len(), 1);
    e.emit_path([0.0; 3], [0.0, 0.0, 1.0], [0.0; 3], 16, &mut rng, &mut live);
    e.emit_path([0.0; 3], [0.0, 0.0, 1.0], [0.0; 3], 16, &mut rng, &mut live);
    assert_eq!(e.particles.len(), 2);
}

/// A particle whose texture does not load draws nothing (not untextured squares); one with no texture named draws as before.
#[test]
fn a_particle_whose_texture_is_missing_is_not_drawn_as_a_flat_square() {
    struct Empty;
    impl Assets for Empty {
        fn read(&self, _: &str) -> Option<Vec<u8>> {
            None
        }
    }
    let mut e = emitter();
    let mut rng = Rng::new(5);
    let mut live = 0;
    e.emit_path(
        [0.0; 3],
        [0.0, 1.0, 0.0],
        [0.0; 3],
        100,
        &mut rng,
        &mut live,
    );
    assert!(!e.particles.is_empty());
    let mut t = TextureSet::new(&Empty);
    e.particle_data[0].textures = vec!["data/particles/missing_smoke".to_string()];
    let _ = take_missing_textures();
    assert!(
        e.draw([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], &mut t, "x")
            .is_none()
    );
    // The name is told once.
    assert_eq!(
        take_missing_textures(),
        vec!["data/particles/missing_smoke".to_string()]
    );
    assert!(take_missing_textures().is_empty());
    e.particle_data[0].textures.clear();
    assert!(
        e.draw([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], &mut t, "x")
            .is_some()
    );
}

#[test]
fn hostile_numbers_stay_bounded() {
    let mut e = emitter();
    e.data.period_ms = 1;
    e.particle_data[0].lifetime_ms = 1 << 20;
    let mut rng = Rng::new(3);
    let mut live = MAX_PARTICLES - 5;
    e.emit_path(
        [f32::NAN; 3],
        [0.0; 3],
        [f32::INFINITY; 3],
        2_000,
        &mut rng,
        &mut live,
    );
    assert!(e.particles.len() <= 5);
    e.emit_burst(
        [0.0; 3],
        [f32::NAN; 3],
        f32::NAN,
        u32::MAX,
        &mut rng,
        &mut live,
    );
    assert!(live <= MAX_PARTICLES);
    struct Empty;
    impl Assets for Empty {
        fn read(&self, _: &str) -> Option<Vec<u8>> {
            None
        }
    }
    let mut t = TextureSet::new(&Empty);
    let _ = e.draw([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], &mut t, "x");
}

/// TGE's datablocks as the client holds them: an emitter of `ParticleEmitterData` 11 makes particles of `ParticleData` 10 with
/// its fields; a missing or wrong-class datablock makes none, and a particle id that names no `ParticleData` is dropped.
#[test]
fn an_emitter_reads_tges_datablocks_from_the_records() {
    use ghosts::Value;
    let mut w = ClientWorld::<TgeClientSet>::default();
    w.datablocks.insert(
        10,
        datablock(
            "ParticleData",
            &[
                ("lifetimeMS", Value::Int(400)),
                ("gravityCoefficient", Value::F32(0.5)),
                ("useInvAlpha", Value::Bool(true)),
                ("textureNameList", Value::str("data/particles/smoke")),
            ],
        ),
    );
    w.datablocks.insert(
        11,
        datablock(
            "ParticleEmitterData",
            &[
                ("ejectionPeriodMS", Value::Int(20)),
                ("ejectionVelocity", Value::F32(3.0)),
                ("dataBlockIds", Value::Int(10)),
            ],
        ),
    );
    w.datablocks.insert(
        12,
        datablock("ParticleEmitterData", &[("dataBlockIds", Value::Int(11))]),
    );
    let p = w.particle_data(10).unwrap();
    assert_eq!(
        (p.lifetime_ms, p.gravity, p.inv_alpha, p.fps),
        (400, 0.5, true, 1)
    );
    assert_eq!(p.textures, vec!["data/particles/smoke".to_string()]);
    let e = Emitter::new(&w, 11).unwrap();
    assert_eq!(
        (e.data.period_ms, e.data.velocity, e.data.particles.clone()),
        (20, 3.0, vec![10])
    );
    assert_eq!(e.particle_data.len(), 1);
    // An emitter whose only particle id names an emitter, an unknown id, a ParticleData asked for as an emitter: none.
    assert!(Emitter::new(&w, 12).is_none());
    assert!(Emitter::new(&w, 99).is_none());
    assert!(w.emitter_data(10).is_none());
    // A hostile period and lifetime are clamped.
    w.datablocks.insert(
        13,
        datablock(
            "ParticleEmitterData",
            &[
                ("ejectionPeriodMS", Value::Int(-5)),
                ("lifetimeMS", Value::Int(i64::MAX)),
                ("dataBlockIds", Value::Int(10)),
            ],
        ),
    );
    let d = w.emitter_data(13).unwrap();
    assert_eq!((d.period_ms, d.lifetime_ms), (1, 1 << 24));
}

/// An emitter's own colours draw in place of its particles' with `useEmitterColors`.
#[test]
fn an_emitters_own_colours_replace_its_particles() {
    let mut e = emitter();
    e.data.use_emitter_colors = true;
    e.set_colors([[0.0, 1.0, 0.0, 1.0]; 4], true);
    let mut rng = Rng::new(9);
    let mut live = 0;
    e.emit_path([0.0; 3], [0.0, 0.0, 1.0], [0.0; 3], 30, &mut rng, &mut live);
    assert!(!e.particles.is_empty());
    // (A particle takes its colour when it is stepped; the one a call made waits a frame.)
    e.advance(10, &mut live);
    e.advance(10, &mut live);
    assert!(
        e.particles
            .iter()
            .all(|q| q.color[1] == 1.0 && q.color[0] == 0.0),
        "{:?}",
        e.particles[0].color
    );
}

/// An axis within 0.9 of z takes TGE's perpendicular (`axis x (0,1,0)`) unless the datablocks name another; theta turns the
/// particle about it.
#[test]
fn an_axis_near_z_turns_about_tges_perpendicular() {
    struct Near(Option<[f32; 3]>);
    impl ParticleDatablocks for Near {
        fn particle_data(&self, _: u32) -> Option<ParticleData> {
            Some(data().1)
        }
        fn emitter_data(&self, _: u32) -> Option<EmitterData> {
            let mut d = data().0;
            (d.theta_min, d.theta_max) = (90.0, 90.0);
            Some(d)
        }
        fn near_z_cross(&self) -> [f32; 3] {
            self.0.unwrap_or([0.0, 1.0, 0.0])
        }
    }
    let first_vel = |w: &Near| {
        let mut e = Emitter::new(w, 1).unwrap();
        let (mut rng, mut live) = (Rng::new(3), 0);
        e.emit_path([0.0; 3], [0.0, 0.0, 1.0], [0.0; 3], 10, &mut rng, &mut live);
        e.particles[0].vel
    };
    // (0,0,1) x (0,1,0) is along x: z turned 90 degrees about it lies along y.
    let v = first_vel(&Near(None));
    assert!(
        (v[1].abs() - 2.0).abs() < 1e-4 && v[0].abs() < 1e-4,
        "{v:?}"
    );
    // (0,0,1) x (1,0,0) is along y: z turned about it lies along x.
    let v = first_vel(&Near(Some([1.0, 0.0, 0.0])));
    assert!(
        (v[0].abs() - 2.0).abs() < 1e-4 && v[1].abs() < 1e-4,
        "{v:?}"
    );
}

/// A set runs at most MAX_EMITTERS emitters, numbered from 1; one removed frees its place.
#[test]
fn a_set_holds_at_most_max_emitters() {
    let mut set = Emitters::default();
    for k in 0..MAX_EMITTERS {
        assert_eq!(set.add(emitter()), Some(k + 1));
    }
    assert_eq!(set.add(emitter()), None);
    assert_eq!(set.len(), MAX_EMITTERS);
    set.retain(|id, _| *id != 1);
    assert_eq!(set.add(emitter()), Some(MAX_EMITTERS + 1));
    assert!(set.get(&1).is_none() && set.get(&2).is_some());
}

/// `framesPerSec` past 200 (or negative) is 20, as TGE's `onAdd` makes it.
#[test]
fn frames_per_second_past_200_is_20() {
    use ghosts::Value;
    let mut w = ClientWorld::<TgeClientSet>::default();
    for (id, fps, want) in [(1, 30, 30), (2, 201, 20), (3, -1, 20)] {
        w.datablocks.insert(
            id,
            datablock("ParticleData", &[("framesPerSec", Value::Int(fps))]),
        );
        assert_eq!(w.particle_data(id).unwrap().fps, want);
    }
}
