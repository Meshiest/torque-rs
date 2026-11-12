use super::*;
use crate::particles::{GpuSpawn, Lit, Orient, SpriteFrame, SpriteLook};

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
        lit: Lit::default(),
        soft_distance: 0.4,
        flipbook_blend: false,
        size_by_velocity: 0.0,
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
    s.push(&frame, |t| Some(10 + t), &mut TextureTable::default(), None);
    assert_eq!(s.records.len(), 3);
    assert_eq!(s.buckets.len(), 3);
    assert_eq!(s.records[2].buckets, vec![2, 0]);
    assert_eq!(
        s.spawns.iter().map(|x| x.look).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    let generation = s.records_generation;
    s.push(&frame, |t| Some(10 + t), &mut TextureTable::default(), None);
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
    s.push(
        &frame(vec![spawn(0, 50), spawn(1, 200)], 0),
        Some,
        &mut TextureTable::default(),
        None,
    );
    assert_eq!(s.live_buckets().count(), 2);
    s.push(
        &frame(Vec::new(), 60),
        Some,
        &mut TextureTable::default(),
        None,
    );
    assert_eq!(
        s.live_buckets().map(|(i, _)| i).collect::<Vec<_>>(),
        vec![1]
    );
    s.push(
        &frame(Vec::new(), 200),
        Some,
        &mut TextureTable::default(),
        None,
    );
    assert_eq!(s.live_buckets().count(), 0);
    s.push(
        &frame(vec![spawn(0, 500)], 0),
        Some,
        &mut TextureTable::default(),
        None,
    );
    s.push(
        &SpriteFrame {
            clear: true,
            ..frame(Vec::new(), 16)
        },
        Some,
        &mut TextureTable::default(),
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
    s.push(&frame, |_| None, &mut TextureTable::default(), None);
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
        &mut TextureTable::default(),
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
        &mut TextureTable::default(),
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
    s.push(
        &frame,
        Some,
        &mut TextureTable::default(),
        Some(ParticleLight::BLENDED),
    );
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
    s.push(&distinct(0), Some, &mut TextureTable::default(), None);
    assert_eq!((s.in_use().0, s.overflow), (MAX_RECORDS, 0));
    // While they live a new look has no room.
    s.push(
        &distinct(MAX_RECORDS),
        Some,
        &mut TextureTable::default(),
        None,
    );
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
        &mut TextureTable::default(),
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
        &mut TextureTable::default(),
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
        &mut TextureTable::default(),
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
    s.push(&frame, Some, &mut TextureTable::default(), None);
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
        &mut TextureTable::default(),
        None,
    );
    assert_eq!(s.wind, [0.0; 3]);
}

/// A table holding `n` solid 4 x 4 textures, the i-th of gray level 40 i.
fn table_of(n: usize) -> TextureTable {
    let mut t = TextureTable::default();
    for i in 0..n {
        t.intern(crate::Texture {
            path: format!("frame{i}"),
            width: 4,
            height: 4,
            levels: std::sync::Arc::new(vec![vec![40 * i as u8; 64], vec![40 * i as u8; 16]]),
        });
    }
    t
}

/// The record a look makes of itself in a table of four frames.
fn pushed(look: SpriteLook, rule: Option<ParticleLight>) -> (GpuSprites, TextureTable) {
    let mut s = GpuSprites::default();
    let mut table = table_of(4);
    let frame = SpriteFrame {
        looks: vec![look],
        spawns: vec![spawn(0, 100)],
        ..SpriteFrame::default()
    };
    s.push(&frame, Some, &mut table, rule);
    (s, table)
}

#[test]
fn a_flipbook_is_one_bucket_over_an_atlas_the_table_holds() {
    let mut l = look(Blend::Alpha, vec![0, 1, 2]);
    l.flipbook_blend = true;
    let (s, table) = pushed(l, None);
    // One bucket, the atlas's texture (the table's fifth), named by every frame.
    assert_eq!(s.buckets.len(), 1);
    assert_eq!(s.records[0].buckets, vec![0, 0, 0]);
    assert_eq!(s.buckets[0].texture.map(|t| t.0), Some(4));
    assert_eq!(table.textures.len(), 5);
    assert_eq!(s.records[0].atlas, Some([2, 2]));
    // The record's flip words: the grid and the blend, then the soft distance and the stretch.
    let mut bytes = Vec::new();
    s.records[0].write(&mut bytes);
    let f = |i: usize| f32::from_le_bytes(bytes[160 + 4 * i..164 + 4 * i].try_into().unwrap());
    assert_eq!([f(0), f(1), f(2), f(3)], [2.0, 2.0, 1.0, 0.0]);
    assert_eq!([f(4), f(5)], [0.4, 0.0]);
}

#[test]
fn a_flipbook_without_blend_asks_for_none_and_one_that_cannot_make_an_atlas_keeps_a_bucket_a_frame()
{
    let (s, _) = pushed(look(Blend::Alpha, vec![0, 1]), None);
    let mut bytes = Vec::new();
    s.records[0].write(&mut bytes);
    assert_eq!(f32::from_le_bytes(bytes[168..172].try_into().unwrap()), 0.0);
    // Frames the table does not hold: no atlas, each frame its own bucket as before.
    let (s, table) = pushed(look(Blend::Alpha, vec![7, 8]), None);
    assert_eq!(s.records[0].atlas, None);
    assert_eq!(s.buckets.len(), 2);
    assert_eq!(table.textures.len(), 4);
}

#[test]
fn a_look_that_does_not_animate_keeps_its_first_frame_alone() {
    let mut l = look(Blend::Alpha, vec![0, 1, 2]);
    l.fps = 0;
    let (s, table) = pushed(l, None);
    assert_eq!(s.records[0].atlas, None);
    assert_eq!(s.records[0].buckets.len(), 1);
    assert_eq!(table.textures.len(), 4);
}

#[test]
fn a_looks_lit_field_beats_the_rule_either_way() {
    let rule = Some(ParticleLight::BLENDED);
    let lit = |l: Lit, rule| {
        let mut x = look(Blend::Alpha, Vec::new());
        x.lit = l;
        let (s, _) = pushed(x, rule);
        s.records[0].material & material::LIT != 0
    };
    assert_eq!(
        [
            lit(Lit::Auto, rule),
            lit(Lit::On, rule),
            lit(Lit::Off, rule)
        ],
        [true, true, false]
    );
    assert_eq!(
        [
            lit(Lit::Auto, None),
            lit(Lit::On, None),
            lit(Lit::Off, None)
        ],
        [false, true, false]
    );
}

#[test]
fn blended_fast_thin_looks_draw_after_the_anti_aliasing_and_others_do_not() {
    let late = |blend, by_velocity: f32| {
        let mut x = look(blend, Vec::new());
        x.size_by_velocity = by_velocity;
        let (s, _) = pushed(x, None);
        s.buckets[0].late
    };
    assert!(late(Blend::Alpha, 0.05));
    assert!(!late(Blend::Alpha, 0.0));
    // An added look is drawn after the anti-aliasing whatever it is: no second bucket for it.
    assert!(!late(Blend::Additive, 0.05));
}

#[test]
fn the_soft_distance_and_the_stretch_reach_the_record() {
    let mut x = look(Blend::Alpha, Vec::new());
    x.soft_distance = 2.5;
    x.size_by_velocity = 0.1;
    let (s, _) = pushed(x, None);
    let mut bytes = Vec::new();
    s.records[0].write(&mut bytes);
    let f = |i: usize| f32::from_le_bytes(bytes[176 + 4 * i..180 + 4 * i].try_into().unwrap());
    assert_eq!([f(0), f(1)], [2.5, 0.1]);
}
