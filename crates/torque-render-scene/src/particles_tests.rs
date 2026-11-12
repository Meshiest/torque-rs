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
            extras: EmitterExtras::default(),
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
            extras: ParticleExtras::default(),
        },
    )
}

fn emitter() -> Emitter {
    let (d, p) = data();
    Emitter {
        data: d,
        particle_data: vec![p],
        particles: Vec::new(),
        handed: Vec::new(),
        next_ms: 0,
        clock_ms: 0,
        last: None,
        near_z_cross: [0.0, 1.0, 0.0],
        colors: None,
        colors_inv_alpha: false,
        wind: [0.0; 3],
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
        fields: ghosts::datablocks::fields::fields(
            &record,
            ghosts::codec::NetWidths::new(10, 10).with_extensions(true),
        ),
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

/// The engine protocol's fields of the particle datablocks come through as the server sent them, and a datablock that sent none
/// has the defaults that leave TGE's look and motion as they are.
#[test]
fn an_emitter_reads_the_engine_fields_the_server_sent_and_defaults_the_rest() {
    use ghosts::Value;
    use ghosts::datablocks::TgeDatablockRecord as R;
    use ghosts::datablocks::effects::{pack_particle_data, pack_particle_emitter_data};
    let f = |name, v: f32| (name, 0, Value::F32(v));
    let mut w = ClientWorld::<TgeClientSet>::default();
    w.datablocks.insert(
        10,
        sent(
            |v| R::ParticleData(pack_particle_data(v)),
            &[
                f("turbulence", 4.0),
                f("turbulenceScale", 2.5),
                ("collide", 0, Value::Bool(true)),
                f("bounceElasticity", 0.6),
                f("bounceFriction", 0.1),
                ("lit", 0, Value::Int(2)),
                f("softDistance", 1.5),
                ("flipbookBlend", 0, Value::Bool(true)),
                f("sizeByVelocity", 0.05),
            ],
        ),
    );
    w.datablocks
        .insert(14, sent(|v| R::ParticleData(pack_particle_data(v)), &[]));
    let emitter = |extra: &[(&str, usize, Value)]| {
        let mut fields = vec![("dataBlockIds", 0, Value::Int(10))];
        fields.extend_from_slice(extra);
        sent(
            |v| R::ParticleEmitterData(pack_particle_emitter_data(v)),
            &fields,
        )
    };
    let bool_on = |name| (name, 0, Value::Bool(true));
    w.datablocks.insert(
        11,
        emitter(&[
            bool_on("trail"),
            ("budget", 0, Value::Int(500)),
            bool_on("orientParticles"),
            bool_on("orientOnVelocity"),
        ]),
    );
    w.datablocks
        .insert(12, emitter(&[bool_on("orientParticles")]));
    w.datablocks.insert(13, emitter(&[]));
    let p = w.particle_data(10).unwrap().extras;
    assert_eq!(
        (p.turbulence, p.turbulence_scale, p.collide),
        (4.0, 2.5, true)
    );
    assert_eq!((p.bounce_elasticity, p.bounce_friction), (0.6, 0.1));
    assert_eq!(
        (p.lit, p.soft_distance, p.flipbook_blend, p.size_by_velocity),
        (Lit::Off, 1.5, true, 0.05)
    );
    // No field sent, none read: the defaults, which are TGE's look.
    assert_eq!(
        w.particle_data(14).unwrap().extras,
        ParticleExtras::default()
    );
    let d = ParticleExtras::default();
    assert_eq!(
        (
            d.turbulence,
            d.collide,
            d.lit,
            d.soft_distance,
            d.size_by_velocity
        ),
        (0.0, false, Lit::Auto, 0.4, 0.0)
    );
    let e = w.emitter_data(11).unwrap().extras;
    assert_eq!((e.trail, e.budget, e.orient), (true, 500, Orient::Velocity));
    assert_eq!(w.emitter_data(12).unwrap().extras.orient, Orient::Emission);
    assert_eq!(w.emitter_data(13).unwrap().extras, EmitterExtras::default());
}

/// A datablock the world holds that the wire's reader did not make (or one it made before a value was out of range) is held to
/// its ranges as it is read.
#[test]
fn hostile_engine_fields_are_held_to_their_ranges_as_they_are_read() {
    let f = |name: &'static str, v: f32| (name, Value::F32(v));
    let mut w = ClientWorld::<TgeClientSet>::default();
    w.datablocks.insert(
        1,
        datablock(
            "ParticleData",
            &[
                f("turbulence", f32::NAN),
                f("turbulenceScale", -5.0),
                f("bounceElasticity", f32::INFINITY),
                f("bounceFriction", 9.0),
                ("lit", Value::Int(77)),
                f("softDistance", f32::NEG_INFINITY),
                f("sizeByVelocity", 1.0e9),
            ],
        ),
    );
    let p = w.particle_data(1).unwrap().extras;
    let d = ParticleExtras::default();
    assert_eq!(p.turbulence, d.turbulence);
    assert_eq!(p.turbulence_scale, 0.01);
    assert_eq!(p.bounce_elasticity, d.bounce_elasticity);
    assert_eq!(p.bounce_friction, 1.0);
    assert_eq!(p.lit, Lit::Auto);
    assert_eq!(p.soft_distance, d.soft_distance);
    assert_eq!(p.size_by_velocity, 10.0);
    // A negative lit and a float lit are no mode either.
    w.datablocks
        .insert(2, datablock("ParticleData", &[("lit", Value::Int(-1))]));
    assert_eq!(w.particle_data(2).unwrap().extras.lit, Lit::Auto);
    for budget in [i64::MAX, i64::from(u32::MAX) + 1, 1 << 40] {
        w.datablocks.insert(
            3,
            datablock(
                "ParticleEmitterData",
                &[
                    ("budget", Value::Int(budget)),
                    ("dataBlockIds", Value::Int(1)),
                ],
            ),
        );
        let budget = w.emitter_data(3).unwrap().extras.budget;
        assert!(budget <= ghosts::datablocks::effects::MAX_EMITTER_BUDGET);
    }
    w.datablocks.insert(
        3,
        datablock(
            "ParticleEmitterData",
            &[("budget", Value::Int(-4)), ("dataBlockIds", Value::Int(1))],
        ),
    );
    assert_eq!(w.emitter_data(3).unwrap().extras.budget, 0);
}

/// An emitter that makes one still, dragless particle of `wind_coefficient` and runs it for `ms` under `wind`: where it ended.
fn drift(wind: [f32; 3], wind_coefficient: f32, ms: u32) -> [f32; 3] {
    let mut e = emitter();
    e.data.velocity = 0.0;
    e.data.velocity_variance = 0.0;
    e.data.offset = 0.0;
    e.particle_data[0].drag = 0.0;
    e.particle_data[0].gravity = 0.0;
    e.particle_data[0].wind = wind_coefficient;
    e.particle_data[0].lifetime_ms = 60_000;
    e.particle_data[0].lifetime_variance_ms = 0;
    e.set_wind(wind);
    let (mut rng, mut live) = (Rng::new(5), 0);
    e.emit_burst(
        [1.0, 2.0, 3.0],
        [0.0, 0.0, 1.0],
        0.0,
        1,
        &mut rng,
        &mut live,
    );
    assert_eq!(e.particles.len(), 1);
    for _ in 0..ms / 10 {
        e.advance(10, &mut live);
    }
    let q = &e.particles[0];
    assert_eq!(
        q.vel, [0.0; 3],
        "the air does not push the particle's own velocity"
    );
    [q.pos[0] - 1.0, q.pos[1] - 2.0, q.pos[2] - 3.0]
}

/// `windCoefficient` 1 under a wind of (3, 0, 0) carries a dragless particle 3 units a second; its coefficient scales that.
#[test]
fn the_wind_carries_a_dragless_particle_its_coefficient_of_the_winds_speed() {
    let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3);
    assert!(near(drift([3.0, 0.0, 0.0], 1.0, 1000), [3.0, 0.0, 0.0]));
    assert!(near(drift([3.0, 0.0, 0.0], 1.0, 500), [1.5, 0.0, 0.0]));
    assert!(near(drift([3.0, 0.0, 0.0], 0.5, 1000), [1.5, 0.0, 0.0]));
    assert!(near(drift([0.0, -2.0, 1.0], 1.0, 1000), [0.0, -2.0, 1.0]));
    // Still air, and a coefficient of 0, carry nothing; a wind that is not finite is still air.
    assert!(near(drift([0.0; 3], 1.0, 1000), [0.0; 3]));
    assert!(near(drift([3.0, 0.0, 0.0], 0.0, 1000), [0.0; 3]));
    assert!(near(
        drift([f32::NAN, 1.0, f32::INFINITY], 1.0, 1000),
        [0.0; 3]
    ));
}

/// A set gives its wind to the emitters it holds and to the ones it takes later.
#[test]
fn a_set_of_emitters_shares_one_wind() {
    let mut set = Emitters::default();
    let first = set.add(emitter()).unwrap();
    set.set_wind([3.0, 0.0, 0.0]);
    let second = set.add(emitter()).unwrap();
    for id in [first, second] {
        assert_eq!(set.get(&id).unwrap().wind, [3.0, 0.0, 0.0]);
    }
}

/// The unbounded floats of a particle datablock are held to ranges as they are read: a coefficient that is not finite is the
/// default, a huge one is cut (TGE refuses a `spinSpeed` outside `-10000..=10000`).
#[test]
fn hostile_wind_acceleration_and_spin_are_held_to_their_ranges() {
    let f = |name: &'static str, v: f32| (name, Value::F32(v));
    let mut w = ClientWorld::<TgeClientSet>::default();
    w.datablocks.insert(
        1,
        datablock(
            "ParticleData",
            &[
                f("windCoefficient", 1.0e30),
                f("constantAcceleration", -1.0e30),
                f("spinSpeed", 1.0e30),
            ],
        ),
    );
    let p = w.particle_data(1).unwrap();
    assert_eq!(
        (p.wind, p.constant_accel, p.spin_speed),
        (
            MAX_WIND_COEFFICIENT,
            -MAX_CONSTANT_ACCELERATION,
            MAX_SPIN_SPEED
        )
    );
    w.datablocks.insert(
        2,
        datablock(
            "ParticleData",
            &[
                f("windCoefficient", f32::NAN),
                f("constantAcceleration", f32::INFINITY),
                f("spinSpeed", f32::NEG_INFINITY),
            ],
        ),
    );
    let p = w.particle_data(2).unwrap();
    assert_eq!((p.wind, p.constant_accel, p.spin_speed), (1.0, 0.0, 0.0));
    w.datablocks.insert(
        3,
        datablock("ParticleData", &[f("windCoefficient", -1.0e9)]),
    );
    assert_eq!(w.particle_data(3).unwrap().wind, -MAX_WIND_COEFFICIENT);
    // A lit that is a float is no mode.
    w.datablocks
        .insert(4, datablock("ParticleData", &[f("lit", 1.0)]));
    assert_eq!(w.particle_data(4).unwrap().extras.lit, Lit::Auto);
}

/// A particle the call outlives is dropped and taken off the live count, as the one that died of age is.
#[test]
fn a_particle_dropped_at_birth_does_not_leak_the_live_count() {
    let mut e = emitter();
    e.data.period_ms = 1;
    e.data.period_variance_ms = 0;
    e.particle_data[0].lifetime_ms = 0;
    e.particle_data[0].lifetime_variance_ms = 0;
    let (mut rng, mut live) = (Rng::new(1), 0);
    for _ in 0..50 {
        e.emit_path(
            [0.0; 3],
            [0.0, 0.0, 1.0],
            [0.0; 3],
            100,
            &mut rng,
            &mut live,
        );
        e.advance(100, &mut live);
        assert_eq!(live, e.particles.len());
    }
    assert!(live <= 1, "{live}");
    assert!(e.particles.len() <= 1);
}

/// The most an emitter holds at once is its `budget`, kept as its particles live on the CPU path and while the pool holds the
/// ones it handed over; a budget of 0 is no share of its own.
#[test]
fn an_emitters_budget_bounds_the_particles_it_holds_on_either_path() {
    struct NoFiles;
    impl Assets for NoFiles {
        fn read(&self, _: &str) -> Option<Vec<u8>> {
            None
        }
    }
    let (mut rng, mut live) = (Rng::new(3), 0);
    let held = |e: &Emitter| e.particles.len() + e.handed.len();
    for budget in [0u32, 5, 12] {
        let mut e = emitter();
        e.data.extras.budget = budget;
        e.particle_data[0].lifetime_ms = 400;
        let mut peak = 0;
        for k in 0..60 {
            e.emit_path([0.0; 3], [0.0, 0.0, 1.0], [0.0; 3], 10, &mut rng, &mut live);
            e.advance(10, &mut live);
            if k % 2 == 1 {
                let mut out = SpriteFrame::default();
                let mut set = crate::textures::TextureSet::new(&NoFiles);
                e.hand_over(&mut out, &mut set, "x", &mut live);
            }
            peak = peak.max(held(&e));
        }
        match budget {
            0 => assert!(peak > 12, "no budget: {peak} held"),
            n => assert_eq!(peak, n as usize, "a budget of {n}"),
        }
    }
    // A particle's room comes back when it dies: after its life the emitter emits again.
    let mut e = emitter();
    e.data.extras.budget = 3;
    e.particle_data[0].lifetime_ms = 100;
    for _ in 0..40 {
        e.emit_path([0.0; 3], [0.0, 0.0, 1.0], [0.0; 3], 10, &mut rng, &mut live);
        e.advance(10, &mut live);
    }
    assert!(!e.particles.is_empty() && e.particles.len() <= 3);
}
