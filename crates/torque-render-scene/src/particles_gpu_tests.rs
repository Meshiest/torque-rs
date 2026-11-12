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
