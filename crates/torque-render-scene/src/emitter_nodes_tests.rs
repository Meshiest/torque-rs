use ghost_world::tge::TgeClientSet;
use ghost_world::world::{ClientWorld, Ghost};
use ghosts::Value;
use ghosts::codec::F32;
use ghosts::fx::ParticleEmitterNode;
use ghosts::parts::GameBase;
use ghosts::tge::{TgeClass, TgeRecord};

use super::*;
use crate::Assets;
use crate::world::tests::datablock;

struct NoFiles;
impl Assets for NoFiles {
    fn read(&self, _: &str) -> Option<Vec<u8>> {
        None
    }
}

/// A datablock as the server packs it from script values `fields` (name, element, value).
fn sent(
    record: impl Fn(&dyn ghosts::ObjectView) -> ghosts::datablocks::TgeDatablockRecord,
    fields: &[(&str, usize, Value)],
) -> ghost_world::tge::TgeDatablock {
    struct View<'a>(&'a [(&'a str, usize, Value)]);
    impl ghosts::ObjectView for View<'_> {
        fn get(&self, field: &str, index: usize) -> Value {
            self.0
                .iter()
                .find(|(n, i, _)| *n == field && *i == index)
                .map_or(Value::Null, |f| f.2.clone())
        }
    }
    let record = record(&View(fields));
    ghost_world::tge::TgeDatablock {
        class: record.class().name().to_string(),
        fields: ghosts::datablocks::fields::fields(&record, ghosts::codec::NetWidths::new(10, 10)),
    }
}

/// The datablocks: particles 10 (a second of life), an emitter 11 of them with one every `period_ms`, and node data 20 with
/// `time_multiple`.
fn world(period_ms: i64, time_multiple: f32) -> ClientWorld<TgeClientSet> {
    world_living(period_ms, time_multiple, 1000)
}

/// [`world`] with particles that live `life_ms`.
fn world_living(period_ms: i64, time_multiple: f32, life_ms: i64) -> ClientWorld<TgeClientSet> {
    use ghosts::datablocks::TgeDatablockRecord as R;
    use ghosts::datablocks::effects::{pack_particle_data, pack_particle_emitter_data};
    let mut w = ClientWorld::<TgeClientSet>::default();
    w.datablocks.insert(
        10,
        sent(
            |v| R::ParticleData(pack_particle_data(v)),
            &[("lifetimeMS", 0, Value::Int(life_ms))],
        ),
    );
    w.datablocks.insert(
        11,
        sent(
            |v| R::ParticleEmitterData(pack_particle_emitter_data(v)),
            &[
                ("ejectionPeriodMS", 0, Value::Int(period_ms)),
                ("ejectionVelocity", 0, Value::F32(2.0)),
                ("dataBlockIds", 0, Value::Int(10)),
            ],
        ),
    );
    w.datablocks.insert(
        20,
        datablock(
            "ParticleEmitterNodeData",
            &[("timeMultiple", Value::F32(time_multiple))],
        ),
    );
    w
}

/// A matrix at (5, 6, 7) turned by nothing (the raw floats a record carries: rows, the place in the fourth column).
fn matrix_at(at: [f32; 3]) -> [F32; 16] {
    let mut m = [0.0; 16];
    for k in 0..4 {
        m[k * 5] = 1.0;
    }
    m[3] = at[0];
    m[7] = at[1];
    m[11] = at[2];
    m.map(F32::new)
}

fn node(emitter: Option<u32>, matrix: [F32; 16]) -> Ghost<TgeClientSet> {
    Ghost {
        class: Some(TgeClass::ParticleEmitterNode),
        serial: 1,
        always: false,
        record: Some(TgeRecord::ParticleEmitterNode(ParticleEmitterNode {
            game: GameBase {
                scale: None,
                datablock: Some(20),
            },
            matrix,
            scale: [F32::new(1.0); 3],
            emitter,
        })),
        part: (),
        position: None,
        updates: 0,
    }
}

/// Runs `frames` frames of `dt` seconds and counts the particles handed to the pool.
fn requests(
    nodes: &mut EmitterNodes,
    w: &ClientWorld<TgeClientSet>,
    frames: u32,
    dt: f32,
) -> usize {
    let mut made = 0;
    for _ in 0..frames {
        nodes.update(w, dt);
        let mut frame = SpriteFrame::default();
        nodes.hand_over(&mut frame, &mut TextureSet::new(&NoFiles));
        made += frame.spawns.len();
    }
    made
}

/// A ghosted node makes `1000 / ejectionPeriodMS` particles a second whatever the frame rate, and `timeMultiple` scales its clock.
#[test]
fn a_ghosted_emitter_node_spawns_a_request_every_ejection_period() {
    for period in [10, 16, 50, 100, 250] {
        for (frames, dt) in [(60, 1.0 / 60.0), (30, 1.0 / 30.0), (144, 1.0 / 144.0)] {
            let mut w = world(period, 1.0);
            w.ghosts
                .insert(7, node(Some(11), matrix_at([5.0, 6.0, 7.0])));
            let mut nodes = EmitterNodes::default();
            let made = requests(&mut nodes, &w, frames, dt);
            let want = (1000 / period) as usize;
            assert!(
                made.abs_diff(want) <= 1,
                "period {period} ms at {frames} frames a second made {made}, not {want}"
            );
        }
    }
    // Twice the clock, twice the particles.
    let mut w = world(50, 2.0);
    w.ghosts.insert(7, node(Some(11), matrix_at([0.0; 3])));
    let made = requests(&mut EmitterNodes::default(), &w, 60, 1.0 / 60.0);
    assert!(made.abs_diff(40) <= 1, "{made}");
    // Two nodes make two streams.
    let mut w = world(100, 1.0);
    w.ghosts.insert(7, node(Some(11), matrix_at([0.0; 3])));
    w.ghosts.insert(8, node(Some(11), matrix_at([9.0; 3])));
    let made = requests(&mut EmitterNodes::default(), &w, 60, 1.0 / 60.0);
    assert!(made.abs_diff(20) <= 2, "{made}");
}

/// The particles are born at the node, along its z axis.
#[test]
fn a_nodes_particles_are_born_at_its_place_along_its_axis() {
    let mut w = world(20, 1.0);
    w.ghosts
        .insert(7, node(Some(11), matrix_at([5.0, 6.0, 7.0])));
    let mut nodes = EmitterNodes::default();
    nodes.update(&w, 0.25);
    let mut frame = SpriteFrame::default();
    nodes.hand_over(&mut frame, &mut TextureSet::new(&NoFiles));
    assert!(frame.spawns.len() >= 10);
    for s in &frame.spawns {
        // Straight up from (5, 6, 7) at 2 units a second (the theta range of the datablock is 0 to 90: spread out of the axis).
        assert!(
            (s.pos[0] - 5.0).abs() < 1.0 && (s.pos[1] - 6.0).abs() < 1.0,
            "{:?}",
            s.pos
        );
        assert!(s.pos[2] >= 7.0 - 1e-3 && s.pos[2] < 8.0, "{:?}", s.pos);
    }
}

/// A node with no emitter, one whose emitter the client lacks, or one whose place is not finite makes nothing; a hostile frame
/// time makes a bounded number; a node gone lets its particles run out and is forgotten.
#[test]
fn hostile_and_missing_nodes_make_little_and_a_gone_node_is_forgotten() {
    let mut w = world(1, 1.0);
    w.ghosts.insert(1, node(None, matrix_at([0.0; 3])));
    w.ghosts.insert(2, node(Some(99), matrix_at([0.0; 3])));
    let mut nan = matrix_at([0.0; 3]);
    nan[3] = F32::new(f32::NAN);
    w.ghosts.insert(3, node(Some(11), nan));
    let mut huge = matrix_at([0.0; 3]);
    huge[0] = F32::new(f32::INFINITY);
    w.ghosts.insert(4, node(Some(11), huge));
    let mut nodes = EmitterNodes::default();
    assert_eq!(requests(&mut nodes, &w, 60, 1.0 / 60.0), 0);
    assert_eq!(nodes.particles(), 0);
    // A frame of a billion seconds, or none, or NaN: at most a second's particles at one every millisecond.
    w.ghosts.clear();
    w.ghosts.insert(5, node(Some(11), matrix_at([0.0; 3])));
    for dt in [1.0e9, f32::NAN, f32::INFINITY, -4.0, 0.0] {
        let mut nodes = EmitterNodes::default();
        assert!(requests(&mut nodes, &w, 1, dt) <= 1000, "dt {dt}");
    }
    // The node goes: its particles run out (they live a second), then so does its emitter.
    let mut nodes = EmitterNodes::default();
    for _ in 0..10 {
        nodes.update(&w, 0.05);
    }
    assert!(nodes.particles() > 0);
    assert_eq!(nodes.emitters(), 1);
    w.ghosts.clear();
    nodes.update(&w, 0.1);
    assert_eq!(nodes.emitters(), 1, "its particles are still alive");
    for _ in 0..30 {
        nodes.update(&w, 0.1);
    }
    assert_eq!((nodes.emitters(), nodes.particles()), (0, 0));
}

/// No more than `MAX_NODES` emit.
#[test]
fn the_nodes_that_emit_are_capped() {
    let mut w = world(50, 1.0);
    for i in 0..(MAX_NODES as u32 + 40) {
        w.ghosts.insert(i + 1, node(Some(11), matrix_at([0.0; 3])));
    }
    let mut nodes = EmitterNodes::default();
    nodes.update(&w, 0.1);
    assert_eq!(nodes.emitters(), MAX_NODES);
}

/// The Sky's wind carries a node's particles: a still, dragless particle of `windCoefficient` 1 goes the wind's speed.
#[test]
fn the_wind_carries_the_nodes_particles() {
    use ghosts::datablocks::TgeDatablockRecord as R;
    use ghosts::datablocks::effects::{pack_particle_data, pack_particle_emitter_data};
    let mut w = ClientWorld::<TgeClientSet>::default();
    w.datablocks.insert(
        10,
        sent(
            |v| R::ParticleData(pack_particle_data(v)),
            &[
                ("lifetimeMS", 0, Value::Int(2000)),
                ("windCoefficient", 0, Value::F32(1.0)),
            ],
        ),
    );
    w.datablocks.insert(
        11,
        sent(
            |v| R::ParticleEmitterData(pack_particle_emitter_data(v)),
            &[
                ("ejectionPeriodMS", 0, Value::Int(20)),
                ("dataBlockIds", 0, Value::Int(10)),
            ],
        ),
    );
    w.datablocks.insert(
        20,
        datablock(
            "ParticleEmitterNodeData",
            &[("timeMultiple", Value::F32(1.0))],
        ),
    );
    w.ghosts.insert(7, node(Some(11), matrix_at([0.0; 3])));
    let mut nodes = EmitterNodes::default();
    nodes.set_wind([30.0, 0.0, 0.0]);
    // The emitter is made at the first frame; the wind is the set's.
    nodes.update(&w, 1.0 / 60.0);
    nodes.set_wind([30.0, 0.0, 0.0]);
    for _ in 0..29 {
        nodes.update(&w, 1.0 / 60.0);
    }
    let mut frame = SpriteFrame::default();
    nodes.hand_over(&mut frame, &mut TextureSet::new(&NoFiles));
    // The first particle came at 20 ms and has flown for the rest of the half second at 30 units a second.
    let farthest = frame
        .spawns
        .iter()
        .map(|s| s.pos[0])
        .fold(f32::MIN, f32::max);
    assert!((13.0..15.5).contains(&farthest), "{farthest}");
    assert!(
        frame
            .spawns
            .iter()
            .all(|s| s.pos[0] >= 0.0 && s.vel.iter().all(|v| v.abs() < 0.05))
    );
}

/// A node's data that sets no `timeMultiple` reads 1 (TGE's constructor), a node whose data the client lacks runs at 1, and a
/// value out of TGE's `0.01..=100` or not finite is held to it.
#[test]
fn an_unset_time_multiple_is_one_and_a_missing_node_datablock_runs_at_one() {
    let mut w = world(100, 1.0);
    let with = |value: Option<Value>| {
        let mut w = ClientWorld::<TgeClientSet>::default();
        let fields: Vec<(&str, Value)> = value.map(|v| ("timeMultiple", v)).into_iter().collect();
        w.datablocks
            .insert(20, datablock("ParticleEmitterNodeData", &fields));
        time_multiple(&w, Some(20))
    };
    assert_eq!(with(None), 1.0);
    assert_eq!(with(Some(Value::F32(2.5))), 2.5);
    assert_eq!(with(Some(Value::F32(0.0))), 0.01);
    assert_eq!(with(Some(Value::F32(-3.0))), 0.01);
    assert_eq!(with(Some(Value::F32(1.0e9))), 100.0);
    assert_eq!(with(Some(Value::F32(f32::NAN))), 1.0);
    assert_eq!(with(Some(Value::Int(7))), 1.0);
    assert_eq!(time_multiple(&w, None), 1.0);
    assert_eq!(time_multiple(&w, Some(99)), 1.0);
    // A node whose data is not held makes a particle a period, as one with a multiple of 1 does.
    w.datablocks.remove(&20);
    w.ghosts.insert(7, node(Some(11), matrix_at([0.0; 3])));
    let made = requests(&mut EmitterNodes::default(), &w, 60, 1.0 / 60.0);
    assert!(made.abs_diff(10) <= 1, "{made}");
}

/// Hostile nodes (a particle a millisecond on a clock a hundred times faster) make at most their share of a frame's particles:
/// one node 64, all of them together 1,024.
#[test]
fn a_frames_particles_are_capped_a_node_and_in_all() {
    let mut w = world(1, 100.0);
    w.ghosts.insert(1, node(Some(11), matrix_at([0.0; 3])));
    for dt in [0.016, 0.1, 5.0] {
        let made = requests(&mut EmitterNodes::default(), &w, 1, dt);
        assert!(
            (1..=MAX_SPAWNS_PER_NODE).contains(&made),
            "one node, a frame of {dt} s made {made}"
        );
    }
    for i in 2..=MAX_NODES as u32 {
        w.ghosts.insert(i, node(Some(11), matrix_at([i as f32; 3])));
    }
    let mut nodes = EmitterNodes::default();
    for dt in [0.016, 0.1, 5.0, 0.016, 0.016] {
        let made = requests(&mut nodes, &w, 1, dt);
        assert!(
            made <= MAX_SPAWNS_PER_FRAME,
            "{} nodes, a frame of {dt} s made {made}",
            MAX_NODES
        );
    }
    // A node of an honest period is not held back: ten a second is ten a second at any share.
    let mut honest = world(100, 1.0);
    honest.ghosts.insert(7, node(Some(11), matrix_at([0.0; 3])));
    let made = requests(&mut EmitterNodes::default(), &honest, 60, 1.0 / 60.0);
    assert!(made.abs_diff(10) <= 1, "{made}");
}

/// A particle dropped at birth (its life is shorter than the call's time left) is not left on the count of live particles.
#[test]
fn nodes_whose_particles_die_at_birth_do_not_leak_the_live_count() {
    let mut w = world_living(1, 100.0, 0);
    for i in 1..=MAX_NODES as u32 {
        w.ghosts.insert(i, node(Some(11), matrix_at([i as f32; 3])));
    }
    let mut nodes = EmitterNodes::default();
    for _ in 0..60 {
        nodes.update(&w, 0.1);
        assert_eq!(nodes.live, nodes.particles());
    }
    // Handed to the pool every frame, the count follows the emitters' too.
    for _ in 0..30 {
        nodes.update(&w, 0.1);
        let mut frame = SpriteFrame::default();
        nodes.hand_over(&mut frame, &mut TextureSet::new(&NoFiles));
        assert_eq!(nodes.live, 0);
    }
    w.ghosts.clear();
    nodes.update(&w, 0.1);
    assert_eq!((nodes.live, nodes.emitters()), (0, 0));
}
