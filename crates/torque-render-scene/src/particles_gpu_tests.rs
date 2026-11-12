use crate::Assets;
use crate::particles::*;
use crate::textures::TextureSet;

struct Empty;
impl Assets for Empty {
    fn read(&self, _: &str) -> Option<Vec<u8>> {
        None
    }
}

fn emitter() -> Emitter {
    let data = EmitterData {
        period_ms: 7,
        period_variance_ms: 3,
        velocity: 2.0,
        velocity_variance: 0.5,
        offset: 0.1,
        theta_min: 0.0,
        theta_max: 40.0,
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
        drag: 0.5,
        wind: 0.0,
        gravity: 0.3,
        inherited_vel: 0.0,
        constant_accel: 0.0,
        lifetime_ms: 400,
        lifetime_variance_ms: 100,
        spin_speed: 30.0,
        spin_min: -10.0,
        spin_max: 10.0,
        inv_alpha: true,
        colors: [
            [1.0, 0.5, 0.25, 1.0],
            [1.0, 0.5, 0.25, 0.0],
            [1.0; 4],
            [1.0; 4],
        ],
        sizes: [0.2, 0.4, 1.0, 1.0],
        times: [0.0, 1.0, 2.0, 2.0],
        textures: Vec::new(),
        animate: false,
        fps: 0,
        extras: ParticleExtras::default(),
    };
    Emitter::of_data(data, vec![p]).expect("an emitter")
}

/// A frame: the emitter advanced and run along a short path.
fn run(e: &mut Emitter, rng: &mut Rng, live: &mut usize, k: u32) {
    e.advance(16, live);
    let x = k as f32 * 0.1;
    e.emit_path([x, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3], 16, rng, live);
}

/// Handing the particles over each frame changes nothing of the emission: the same random numbers are drawn and the same
/// particles made, each handed over in the state the CPU path had it in when it was drawn.
#[test]
fn handing_over_keeps_the_emission_and_the_particles_state() {
    let (mut cpu, mut gpu) = (emitter(), emitter());
    let (mut rng_cpu, mut rng_gpu) = (Rng::new(9), Rng::new(9));
    let (mut live_cpu, mut live_gpu) = (0, 0);
    let mut t = TextureSet::new(&Empty);
    let mut handed = 0;
    for k in 0..40 {
        let before = cpu.particles.len();
        run(&mut cpu, &mut rng_cpu, &mut live_cpu, k);
        run(&mut gpu, &mut rng_gpu, &mut live_gpu, k);
        let mut frame = SpriteFrame::default();
        gpu.hand_over(&mut frame, &mut t, "x", &mut live_gpu);
        assert!(gpu.particles.is_empty());
        assert_eq!(live_gpu, 0);
        // The CPU's newest particles are the ones handed over, in the same state.
        let new = &cpu.particles[cpu.particles.len() - frame.spawns.len()..];
        assert!(cpu.particles.len() - frame.spawns.len() <= before);
        for (q, s) in new.iter().zip(&frame.spawns) {
            assert_eq!(
                (q.pos, q.vel, q.age_ms, q.life_ms),
                (s.pos, s.vel, s.age_ms, s.life_ms)
            );
            assert_eq!((q.color, q.size, q.fresh), (s.color, s.size, s.fresh));
        }
        handed += frame.spawns.len();
    }
    assert!(handed > 40);
    assert_eq!(rng_cpu.f(), rng_gpu.f());
}

#[test]
fn a_look_takes_the_emitters_own_colors_and_their_blend() {
    let mut e = emitter();
    let mut t = TextureSet::new(&Empty);
    let own = [[0.0, 1.0, 0.0, 1.0]; 4];
    e.set_colors(own, false);
    // Not used without useEmitterColors.
    let look = e.look(0, &mut t, "x");
    assert_eq!(look.blend, crate::Blend::Alpha);
    assert_eq!(look.colors, e.particle_data[0].colors);
    e.data.use_emitter_colors = true;
    let look = e.look(0, &mut t, "x");
    assert_eq!((look.colors, look.blend), (own, crate::Blend::Additive));
}

#[test]
fn a_look_whose_textures_do_not_load_is_hidden_and_told() {
    let mut e = emitter();
    let mut t = TextureSet::new(&Empty);
    e.particle_data[0].textures = vec!["data/particles/missing_spark".to_string()];
    let _ = take_missing_textures();
    let look = e.look(0, &mut t, "x");
    assert!(look.hidden && look.frames.is_empty());
    assert_eq!(
        take_missing_textures(),
        vec!["data/particles/missing_spark".to_string()]
    );
    e.particle_data[0].textures.clear();
    assert!(!e.look(0, &mut t, "x").hidden);
}

#[test]
fn a_frame_keeps_each_look_once_and_a_spawn_is_64_bytes() {
    let mut e = emitter();
    let (mut rng, mut live) = (Rng::new(3), 0);
    let mut t = TextureSet::new(&Empty);
    let mut frame = SpriteFrame::default();
    for k in 0..4 {
        run(&mut e, &mut rng, &mut live, k);
        e.hand_over(&mut frame, &mut t, "x", &mut live);
    }
    assert_eq!(frame.looks.len(), 1);
    let mut bytes = Vec::new();
    frame.spawns[0].write(&mut bytes);
    assert_eq!(bytes.len(), GpuSpawn::BYTES);
    assert!(!frame.is_empty());
}

#[test]
fn the_named_offsets_find_the_fields_in_a_spawns_bytes() {
    let spawn = GpuSpawn {
        pos: [1.0, 2.0, 3.0],
        vel: [4.0, 5.0, 6.0],
        look: 7,
        ..spawn_of_flags()
    };
    let mut bytes = Vec::new();
    spawn.write(&mut bytes);
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let floats = |at: usize| [0, 4, 8].map(|k| f32::from_bits(word(at + k)));
    assert_eq!(floats(GpuSpawn::POS_AT), spawn.pos);
    assert_eq!(floats(GpuSpawn::VEL_AT), spawn.vel);
    assert_eq!(word(GpuSpawn::LOOK_AT), 7);
    assert_eq!(
        word(GpuSpawn::LOOK_AT + 4) & GpuSpawn::ALIVE,
        GpuSpawn::ALIVE
    );
}

/// The ejection direction the pool's vertex stage unfolds from a particle's flags (`gpu_particle_types.wgsl`'s `pool_orient_dir`).
fn unfolded(flags: u32) -> [f32; 3] {
    let bits = flags >> GpuSpawn::ORIENT_SHIFT;
    let a = (bits & 0x7fff) as f32 / 32767.0 * 2.0 - 1.0;
    let b = ((bits >> 15) & 0x7fff) as f32 / 32767.0 * 2.0 - 1.0;
    let mut n = [a, b, 1.0 - a.abs() - b.abs()];
    if n[2] < 0.0 {
        let sign = |v: f32| if v >= 0.0 { 1.0 } else { -1.0 };
        n = [(1.0 - b.abs()) * sign(a), (1.0 - a.abs()) * sign(b), n[2]];
    }
    let len = n.iter().map(|c| c * c).sum::<f32>().sqrt();
    n.map(|c| c / len)
}

/// A direction travels in the 30 bits above a particle's alive and fresh flags and comes back within a thousandth of a radian,
/// whichever way it points; none (or a direction that is not finite) is up.
#[test]
fn an_ejection_direction_travels_in_the_flags_of_a_particle() {
    let mut rng = Rng::new(11);
    let mut dirs: Vec<[f32; 3]> = vec![
        [1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
        [0.6, -0.8, 0.0],
        [-0.3, 0.2, -0.9],
    ];
    dirs.extend((0..2000).map(|_| [0, 1, 2].map(|_| rng.f() * 2.0 - 1.0)));
    for d in dirs {
        let len = d.iter().map(|c| c * c).sum::<f32>().sqrt();
        if len < 1e-3 {
            continue;
        }
        let u = d.map(|c| c / len);
        let back = unfolded(orient_bits(d) << GpuSpawn::ORIENT_SHIFT);
        let dot: f32 = (0..3).map(|k| u[k] * back[k]).sum();
        assert!(dot > 0.999_999, "{d:?} came back as {back:?}");
    }
    for none in [[0.0; 3], [f32::NAN; 3], [f32::INFINITY, 0.0, 0.0]] {
        assert_eq!(orient_bits(none), orient_bits([0.0, 0.0, 1.0]), "{none:?}");
    }
    // The bits stay above the two flags.
    assert_eq!(orient_bits([0.3, 0.9, -0.2]) >> 30, 0);
    let mut bytes = Vec::new();
    GpuSpawn {
        fresh: true,
        orient_dir: [0.0, 1.0, 0.0],
        ..spawn_of_flags()
    }
    .write(&mut bytes);
    let flags = u32::from_le_bytes(bytes[52..56].try_into().unwrap());
    assert_eq!(flags & 3, GpuSpawn::ALIVE | GpuSpawn::FRESH);
    let up = unfolded(flags);
    assert!(up[1] > 0.9999, "{up:?}");
}

fn spawn_of_flags() -> GpuSpawn {
    GpuSpawn {
        pos: [0.0; 3],
        age_ms: 0,
        vel: [0.0; 3],
        life_ms: 100,
        acc: [0.0; 3],
        spin: 0.0,
        look: 0,
        fresh: false,
        color: [1.0; 4],
        size: 1.0,
        orient_dir: [0.0, 0.0, 1.0],
    }
}

/// A particle keeps the way it was ejected, and its look the way its emitter turns it.
#[test]
fn handing_over_keeps_the_ejection_direction_and_the_emitters_orientation() {
    let mut e = emitter();
    e.data.extras.orient = Orient::of(true, true);
    let (mut rng, mut live) = (Rng::new(2), 0);
    let mut t = TextureSet::new(&Empty);
    let mut frame = SpriteFrame::default();
    for k in 0..4 {
        run(&mut e, &mut rng, &mut live, k);
        let made: Vec<[f32; 3]> = e.particles.iter().map(|q| q.orient_dir).collect();
        let before = frame.spawns.len();
        e.hand_over(&mut frame, &mut t, "x", &mut live);
        let handed: Vec<[f32; 3]> = frame.spawns[before..]
            .iter()
            .map(|s| s.orient_dir)
            .collect();
        assert_eq!(made, handed);
    }
    assert!(frame.spawns.len() > 4);
    for s in &frame.spawns {
        let len = s.orient_dir.iter().map(|c| c * c).sum::<f32>().sqrt();
        assert!((len - 1.0).abs() < 1e-4, "{:?}", s.orient_dir);
    }
    assert_eq!(frame.looks[0].orient, Orient::Velocity);
    let plain = emitter().look(0, &mut t, "x");
    assert_eq!(plain.orient, Orient::Billboard);
}
