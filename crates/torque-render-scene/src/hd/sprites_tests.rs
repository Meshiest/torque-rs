use super::*;
use crate::particles::{GpuSpawn, Orient, SpriteFrame, SpriteLook};

fn look(blend: Blend, frames: Vec<usize>) -> SpriteLook {
    SpriteLook {
        label: "x".into(),
        colors: [[1.0; 4]; 4],
        sizes: [1.0; 4],
        times: [0.0, 1.0, 2.0, 2.0],
        drag: 0.0,
        gravity: 0.0,
        wind: 0.0,
        blend,
        fps: if frames.len() > 1 { 10 } else { 0 },
        frames,
        hidden: false,
        orient: Orient::default(),
    }
}

fn spawn(look: u32, life_ms: i32) -> GpuSpawn {
    GpuSpawn {
        pos: [0.0; 3],
        age_ms: 0,
        vel: [0.0; 3],
        life_ms,
        acc: [0.0; 3],
        spin: 0.0,
        look,
        fresh: false,
        color: [1.0; 4],
        size: 1.0,
        orient_dir: [0.0, 0.0, 1.0],
    }
}

#[test]
fn looks_become_records_once_and_their_frames_buckets() {
    let mut s = GpuSprites::default();
    let frame = SpriteFrame {
        looks: vec![
            look(Blend::Alpha, vec![0]),
            look(Blend::Additive, vec![0]),
            look(Blend::Alpha, vec![1, 0]),
        ],
        spawns: vec![spawn(0, 100), spawn(1, 100), spawn(2, 100)],
        steps: vec![16],
        clear: false,
        wind: [0.0; 3],
    };
    // Frame texture i is the scene's 10 + i.
    s.push(&frame, |t| Some(10 + t), None);
    assert_eq!(s.records.len(), 3);
    assert_eq!(s.buckets.len(), 3);
    assert_eq!(s.records[2].buckets, vec![2, 0]);
    assert_eq!(
        s.spawns.iter().map(|x| x.look).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    let generation = s.records_generation;
    s.push(&frame, |t| Some(10 + t), None);
    assert_eq!((s.records.len(), s.records_generation), (3, generation));
    assert!(s.records[1].material & material::EMITTER != 0);
    assert!(s.records[0].material & material::EMITTER == 0);
    let mut bytes = Vec::new();
    s.records[2].write(&mut bytes);
    assert_eq!(bytes.len(), SpriteRecord::BYTES);
}

#[test]
fn a_bucket_is_drawn_while_a_particle_handed_to_it_can_live() {
    let mut s = GpuSprites::default();
    let frame = |spawns, step_ms: u32| SpriteFrame {
        looks: vec![
            look(Blend::Alpha, vec![0]),
            look(Blend::Additive, Vec::new()),
        ],
        spawns,
        steps: vec![step_ms],
        clear: false,
        wind: [0.0; 3],
    };
    s.push(&frame(vec![spawn(0, 50), spawn(1, 200)], 0), Some, None);
    assert_eq!(s.live_buckets().count(), 2);
    s.push(&frame(Vec::new(), 60), Some, None);
    assert_eq!(
        s.live_buckets().map(|(i, _)| i).collect::<Vec<_>>(),
        vec![1]
    );
    s.push(&frame(Vec::new(), 200), Some, None);
    assert_eq!(s.live_buckets().count(), 0);
    s.push(&frame(vec![spawn(0, 500)], 0), Some, None);
    s.push(
        &SpriteFrame {
            clear: true,
            ..frame(Vec::new(), 16)
        },
        Some,
        None,
    );
    assert_eq!(s.live_buckets().count(), 0);
}

#[test]
fn hidden_looks_and_full_tables_draw_nothing() {
    let mut s = GpuSprites::default();
    let mut hidden = look(Blend::Alpha, Vec::new());
    hidden.hidden = true;
    // A frame texture the scene does not have hides its look too.
    let frame = SpriteFrame {
        looks: vec![hidden, look(Blend::Alpha, vec![7])],
        spawns: vec![spawn(0, 100), spawn(1, 100)],
        ..SpriteFrame::default()
    };
    s.push(&frame, |_| None, None);
    assert_eq!(s.buckets.len(), 1);
    assert!(!s.buckets[0].drawn);
    assert_eq!(s.live_buckets().count(), 0);
    // Past the drawn buckets' room every new look is hidden.
    let looks: Vec<SpriteLook> = (0..80).map(|t| look(Blend::Alpha, vec![t])).collect();
    s.push(
        &SpriteFrame {
            looks,
            ..SpriteFrame::default()
        },
        Some,
        None,
    );
    assert_eq!(s.buckets.len(), MAX_BUCKETS);
    assert!(s.overflow > 0);
    assert_eq!(s.buckets.iter().filter(|b| !b.drawn).count(), 1);
}

#[test]
fn a_frames_steps_are_kept_in_order_and_a_long_stalls_summed_into_the_last() {
    let mut s = GpuSprites::default();
    let mut steps: Vec<u32> = (1..=MAX_STEPS as u32 + 3).collect();
    steps.insert(2, 0);
    s.push(
        &SpriteFrame {
            steps,
            ..SpriteFrame::default()
        },
        Some,
        None,
    );
    assert_eq!(s.steps.len(), MAX_STEPS);
    assert_eq!(&s.steps[..3], &[1, 2, 3]);
    let n = MAX_STEPS as u32;
    assert_eq!(*s.steps.last().unwrap(), n + (n + 1) + (n + 2) + (n + 3));
}

#[test]
fn the_light_rule_marks_records_lit() {
    let mut s = GpuSprites::default();
    let frame = SpriteFrame {
        looks: vec![look(Blend::Alpha, vec![0]), look(Blend::Additive, vec![0])],
        ..SpriteFrame::default()
    };
    s.push(&frame, Some, Some(ParticleLight::BLENDED));
    assert!(s.records[0].material & material::LIT != 0);
    assert!(s.records[1].material & material::LIT == 0);
    assert!(s.buckets[usize::from(s.records[0].buckets[0])].lit);
}

#[test]
fn a_full_table_frees_the_looks_no_live_particle_names_and_a_clear_empties_it() {
    let mut s = GpuSprites::default();
    // MAX_RECORDS looks, each with a particle 100 ms long.
    let distinct = |base: usize| -> SpriteFrame {
        let looks: Vec<SpriteLook> = (0..MAX_RECORDS)
            .map(|k| {
                let mut l = look(Blend::Alpha, Vec::new());
                l.drag = (base + k) as f32;
                l
            })
            .collect();
        let spawns = (0..MAX_RECORDS as u32).map(|k| spawn(k, 100)).collect();
        SpriteFrame {
            looks,
            spawns,
            ..SpriteFrame::default()
        }
    };
    s.push(&distinct(0), Some, None);
    assert_eq!((s.in_use().0, s.overflow), (MAX_RECORDS, 0));
    // While they live a new look has no room.
    s.push(&distinct(MAX_RECORDS), Some, None);
    assert_eq!(s.in_use().0, MAX_RECORDS);
    assert!(s.overflow > 0);
    // Once their particles are dead, new looks take their slots.
    let overflow = s.overflow;
    s.push(
        &SpriteFrame {
            steps: vec![200],
            ..distinct(2 * MAX_RECORDS)
        },
        Some,
        None,
    );
    assert_eq!((s.records.len(), s.overflow), (MAX_RECORDS, overflow));
    assert_eq!(s.spawns.len(), MAX_RECORDS);
    s.push(
        &SpriteFrame {
            clear: true,
            ..SpriteFrame::default()
        },
        Some,
        None,
    );
    assert_eq!(s.in_use(), (0, 0));
    assert!(s.records.is_empty());
}

#[test]
fn looks_that_differ_only_in_label_share_a_record() {
    let mut s = GpuSprites::default();
    let mut other = look(Blend::Alpha, vec![0]);
    other.label = "y".into();
    s.push(
        &SpriteFrame {
            looks: vec![look(Blend::Alpha, vec![0]), other],
            ..SpriteFrame::default()
        },
        Some,
        None,
    );
    assert_eq!(s.records.len(), 1);
}

#[test]
fn a_frames_wind_and_a_looks_orient_mode_reach_the_pool() {
    let mut s = GpuSprites::default();
    let mut turned = look(Blend::Alpha, vec![0]);
    turned.orient = Orient::Velocity;
    let frame = SpriteFrame {
        looks: vec![look(Blend::Alpha, vec![0]), turned],
        spawns: vec![spawn(0, 100), spawn(1, 100)],
        wind: [3.0, 0.0, -1.0],
        ..SpriteFrame::default()
    };
    s.push(&frame, Some, None);
    assert_eq!(s.wind, [3.0, 0.0, -1.0]);
    // Two looks alike but for their orientation are two records, the mode the third word after the keys and coefficients.
    assert_eq!(s.records.len(), 2);
    let word = |r: &SpriteRecord, at: usize| {
        let mut bytes = Vec::new();
        r.write(&mut bytes);
        assert_eq!(bytes.len(), SpriteRecord::BYTES);
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    };
    assert_eq!((word(&s.records[0], 120), word(&s.records[1], 120)), (0, 2));
    // A wind that is not finite is still air.
    s.push(
        &SpriteFrame {
            wind: [f32::NAN, 1.0, 1.0],
            ..SpriteFrame::default()
        },
        Some,
        None,
    );
    assert_eq!(s.wind, [0.0; 3]);
}
