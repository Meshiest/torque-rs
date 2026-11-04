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

/// A datablock as the server's pack writes script values `fields` (name, element, value) and the client's record names what the
/// codec read ([`ghosts::datablocks::fields`]).
fn sent(
    record: impl Fn(&dyn ghosts::ObjectView) -> ghosts::datablocks::TgeDatablockRecord,
    fields: &[(&str, usize, ghosts::Value)],
) -> ghost_world::tge::TgeDatablock {
    struct View<'a>(&'a [(&'a str, usize, ghosts::Value)]);
    impl ghosts::ObjectView for View<'_> {
        fn get(&self, field: &str, index: usize) -> ghosts::Value {
            self.0
                .iter()
                .find(|(n, i, _)| *n == field && *i == index)
                .map_or(ghosts::Value::Null, |f| f.2.clone())
        }
    }
    let record = record(&View(fields));
    ghost_world::tge::TgeDatablock {
        class: record.class().name().to_string(),
        fields: ghosts::datablocks::fields::fields(&record, ghosts::codec::NetWidths::new(10, 10)),
    }
}

/// TGE's datablocks as the client holds them, sent as the server packs them: an emitter of `ParticleEmitterData` 11 makes
/// particles of `ParticleData` 10 with the values its script gave (to what the wire keeps: 32 ms lifetimes, hundredths of a
/// speed, 7-bit colors); a missing or wrong-class datablock makes none, and a particle id that names no `ParticleData` is
/// dropped.
#[test]
fn an_emitter_reads_tges_datablocks_as_the_server_sent_them() {
    use ghosts::Value;
    use ghosts::datablocks::TgeDatablockRecord as R;
    use ghosts::datablocks::effects::{pack_particle_data, pack_particle_emitter_data};
    let f = |name, i, v: f32| (name, i, Value::F32(v));
    let mut w = ClientWorld::<TgeClientSet>::default();
    let particle = [
        f("dragCoefficient", 0, 2.0),
        f("windCoefficient", 0, 1.0),
        f("gravityCoefficient", 0, -0.5),
        f("inheritedVelFactor", 0, 0.25),
        ("lifetimeMS", 0, Value::Int(96)),
        ("lifetimeVarianceMS", 0, Value::Int(32)),
        f("spinRandomMin", 0, -90.0),
        f("spinRandomMax", 0, 90.0),
        ("useInvAlpha", 0, Value::Bool(true)),
        f("colors", 0, 1.0),
        f("colors", 1, 0.5),
        f("colors", 2, 0.0),
        f("colors", 3, 1.0),
        f("colors", 4, 1.0),
        f("colors", 5, 1.0),
        f("colors", 6, 1.0),
        f("colors", 7, 0.0),
        f("sizes", 0, 0.16),
        f("sizes", 1, 0.04),
        f("times", 0, 0.0),
        f("times", 1, 1.0),
        ("textureNameList", 0, Value::str("data/particles/smoke")),
    ];
    w.datablocks.insert(
        10,
        sent(|v| R::ParticleData(pack_particle_data(v)), &particle),
    );
    let emitter = [
        ("ejectionPeriodMS", 0, Value::Int(20)),
        ("periodVarianceMS", 0, Value::Int(5)),
        f("ejectionVelocity", 0, 3.0),
        f("velocityVariance", 0, 0.25),
        f("ejectionOffset", 0, 0.5),
        f("thetaMax", 0, 20.0),
        f("phiVariance", 0, 360.0),
        ("lifetimeMS", 0, Value::Int(640)),
        ("dataBlockIds", 0, Value::Int(10)),
    ];
    w.datablocks.insert(
        11,
        sent(
            |v| R::ParticleEmitterData(pack_particle_emitter_data(v)),
            &emitter,
        ),
    );
    w.datablocks.insert(
        12,
        sent(
            |v| R::ParticleEmitterData(pack_particle_emitter_data(v)),
            &[("dataBlockIds", 0, Value::Int(11))],
        ),
    );
    let near = |a: f32, b: f32, e: f32| (a - b).abs() <= e;
    let p = w.particle_data(10).unwrap();
    assert!(
        near(p.drag, 2.0, 0.01) && near(p.gravity, -0.5, 0.01),
        "{p:?}"
    );
    assert!(near(p.inherited_vel, 0.25, 0.002) && p.wind == 1.0, "{p:?}");
    assert_eq!((p.lifetime_ms, p.lifetime_variance_ms), (96, 32));
    assert_eq!(
        (p.spin_min, p.spin_max, p.inv_alpha, p.fps),
        (-90.0, 90.0, true, 1)
    );
    assert_eq!(p.colors[0][0], 1.0);
    assert!(
        near(p.colors[0][1], 0.5, 0.01) && p.colors[1][3] == 0.0,
        "{p:?}"
    );
    assert!(
        near(p.sizes[0], 0.16, 0.005) && near(p.sizes[1], 0.04, 0.005),
        "{p:?}"
    );
    // Two keys sent (the second at time 1); the others keep the constructor's.
    assert_eq!(p.times, [0.0, 1.0, 2.0, 2.0]);
    assert_eq!((p.colors[2], p.sizes[2]), ([1.0; 4], 1.0));
    assert_eq!(p.textures, vec!["data/particles/smoke".to_string()]);
    let e = Emitter::new(&w, 11).unwrap();
    let d = &e.data;
    assert_eq!(
        (d.period_ms, d.period_variance_ms, d.lifetime_ms),
        (20, 5, 640)
    );
    assert_eq!(
        (d.velocity, d.velocity_variance, d.offset),
        (3.0, 0.25, 0.5)
    );
    assert_eq!(
        (d.theta_min, d.theta_max, d.phi_ref_vel, d.phi_variance),
        (0.0, 20.0, 0.0, 360.0)
    );
    assert_eq!(d.particles, vec![10]);
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
    assert_eq!((d.period_ms, d.lifetime_ms), (1, 1023 << 5));
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

#[test]
fn a_trail_is_emitted_against_the_flight_from_where_the_projectile_was() {
    assert_eq!(trail_axis([0.0, 30.0, 0.0]), [0.0, -1.0, 0.0]);
    assert_eq!(trail_axis([0.0; 3]), [0.0, 0.0, 1.0], "still: up");
    assert_eq!(trail_axis([f32::NAN, 0.0, 0.0]), [0.0, 0.0, 1.0]);
    let mut e = emitter();
    e.data.velocity = 1.0;
    let mut rng = Rng::new(3);
    let mut live = 0;
    // 32 ms from y = 0 to y = 1.6 (50 units a second): a particle every 10 ms along it, each leaving backwards.
    e.emit_trail(
        [0.0; 3],
        [0.0, 1.6, 0.0],
        [0.0, 50.0, 0.0],
        32,
        &mut rng,
        &mut live,
    );
    assert_eq!(e.particles.len(), 3);
    for q in &e.particles {
        assert!(q.pos[1] > 0.0 && q.pos[1] < 1.6, "on the path: {:?}", q.pos);
        assert!(q.vel[1] < 0.0 && q.vel[0].abs() < 1e-5, "{:?}", q.vel);
    }
    // The next call goes on from where this one ended, whatever `from` it names.
    e.emit_trail(
        [9.0; 3],
        [0.0, 3.2, 0.0],
        [0.0, 50.0, 0.0],
        32,
        &mut rng,
        &mut live,
    );
    assert_eq!(e.particles.len(), 6);
    assert!(
        e.particles[3..]
            .iter()
            .all(|q| q.pos[1] > 1.6 - 0.1 && q.pos[0].abs() < 0.1)
    );
}
