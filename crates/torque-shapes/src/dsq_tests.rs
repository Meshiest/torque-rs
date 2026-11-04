use super::*;
use crate::write::write_dsq;

fn free(_: u64) -> bool {
    true
}

fn w(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// A file with two nodes, one rotation, one translation, two sequences and one trigger.
pub(super) fn sample() -> Vec<u8> {
    let mut o = Vec::new();
    w(&mut o, 24);
    w(&mut o, 2);
    for n in [&b"root"[..], b"arm"] {
        w(&mut o, n.len() as u32);
        o.extend_from_slice(n);
    }
    w(&mut o, 0);
    w(&mut o, 7);
    w(&mut o, 1);
    o.extend_from_slice(&[0u8; 8]);
    w(&mut o, 1);
    o.extend_from_slice(&[0u8; 12]);
    w(&mut o, 0); // uniform
    w(&mut o, 0); // aligned
    w(&mut o, 0); // arbitrary
    w(&mut o, 0); // ground
    w(&mut o, 0); // object states
    w(&mut o, 2);
    for n in [&b"run"[..], b"walk"] {
        w(&mut o, n.len() as u32);
        o.extend_from_slice(n);
        for _ in 0..14 {
            w(&mut o, 0);
        }
        for _ in 0..8 {
            w(&mut o, 0);
            w(&mut o, 1);
            w(&mut o, 3);
        }
    }
    w(&mut o, 1);
    w(&mut o, 1);
    w(&mut o, 0x3f80_0000);
    o
}

#[test]
fn the_sample_scans_to_its_last_byte() {
    let bytes = sample();
    let scan = scan(&bytes, &mut free).expect("scans");
    assert_eq!(scan.version, 24);
    assert_eq!(scan.node_names, [b"root".to_vec(), b"arm".to_vec()]);
    assert_eq!((scan.node_rotations, scan.node_translations), (1, 1));
    assert_eq!(scan.sequences.len(), 2);
    assert_eq!(scan.sequences[1].0, b"walk");
    assert_eq!(scan.triggers, 1);
    assert_eq!(scan.trailing, 0);
}

#[test]
fn the_sample_writes_back_to_its_bytes() {
    let bytes = sample();
    let dsq = parse(&bytes, &mut free).expect("parses");
    assert_eq!(dsq.spare, [0, 7]);
    assert_eq!(write_dsq(&dsq), bytes);
    // A trailing byte is kept and written back.
    let mut extra = bytes.clone();
    extra.push(9);
    assert_eq!(write_dsq(&parse(&extra, &mut free).unwrap()), extra);
}

#[test]
fn every_truncation_is_an_error_and_a_long_name_is_kept_whole() {
    let bytes = sample();
    for cut in 0..bytes.len() {
        assert!(scan(&bytes[..cut], &mut free).is_err(), "cut at {cut}");
    }
    let mut extra = bytes.clone();
    extra.push(0);
    assert_eq!(scan(&extra, &mut free).expect("scans").trailing, 1);
    let mut o = Vec::new();
    w(&mut o, 24);
    w(&mut o, 1);
    w(&mut o, 300);
    o.extend(std::iter::repeat_n(b'x', 300));
    // Two leading counts, six keyframe counts, object states, sequences and triggers: eleven zero words.
    for _ in 0..11 {
        w(&mut o, 0);
    }
    let dsq = parse(&o, &mut free).expect("parses");
    assert_eq!(dsq.node_names[0].len(), 300);
    assert_eq!(stored_name(&dsq.node_names[0]).map(<[u8]>::len), Some(255));
    assert_eq!(write_dsq(&dsq), o);
    assert_eq!(scan(&o, &mut free).expect("scans").node_names[0].len(), 255);
}

#[test]
fn versions_counts_and_budgets_are_refused() {
    let mut bytes = sample();
    bytes[0] = 25;
    assert_eq!(scan(&bytes, &mut free), Err(ShapeError::NewerVersion(25)));
    bytes[0] = 17;
    assert_eq!(
        scan(&bytes, &mut free),
        Err(ShapeError::UnsupportedVersion(17))
    );
    let mut huge = sample();
    huge[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        scan(&huge, &mut free),
        Err(ShapeError::Cap { .. })
    ));
    assert_eq!(
        scan(&sample(), &mut |_| false),
        Err(ShapeError::OutOfBudget)
    );
    let big = vec![0u8; MAX_DSQ_BYTES + 1];
    assert!(matches!(
        scan(&big, &mut free),
        Err(ShapeError::TooLarge { .. })
    ));
}

/// A shape with nodes `root` and `arm` and one sequence called `idle`.
fn base_shape() -> Shape {
    let mut shape = Shape::empty();
    shape.names = vec![b"root".to_vec(), b"arm".to_vec(), b"idle".to_vec()];
    let node = |name, parent| crate::shape::Node {
        name,
        parent,
        first_object: -1,
        first_child: -1,
        next_sibling: -1,
    };
    shape.nodes = vec![node(0, -1), node(1, 0)];
    shape.node_rotations = vec![[0; 4]; 2];
    shape.node_translations = vec![[0.0; 3]; 2];
    let mut idle = parse(&sample(), &mut free).unwrap().sequences[0]
        .sequence
        .clone();
    idle.name = 2;
    shape.sequences = vec![idle];
    shape
}

#[test]
fn an_import_appends_keyframes_and_moves_the_bases_and_sets() {
    let mut shape = base_shape();
    let mut dsq = parse(&sample(), &mut free).unwrap();
    // The second sequence moves the file's node 1 (arm) and has a trigger and one scale kind.
    dsq.sequences[1].sequence.rotation_matters = IntSet {
        first: 0,
        words: vec![0b10],
    };
    dsq.sequences[1].sequence.base_rotation = 0;
    dsq.sequences[1].sequence.first_trigger = 0;
    dsq.sequences[1].sequence.flags = SEQ_UNIFORM_SCALE;
    dsq.uniform_scales = vec![1.0, 2.0];
    shape.node_uniform_scales = vec![9.0];
    shape.import_sequences(&dsq, &mut free).expect("imports");
    assert_eq!(shape.sequences.len(), 3);
    assert_eq!(shape.node_rotations.len(), 3);
    assert_eq!(shape.triggers.len(), 1);
    let walk = &shape.sequences[2];
    assert_eq!(shape.names[walk.name as usize], b"walk");
    assert_eq!(shape.names.len(), 5);
    assert_eq!(walk.base_rotation, 2);
    assert_eq!(walk.base_scale, 1);
    assert_eq!(walk.rotation_matters.words, vec![0b10]);
    assert_eq!(walk.translation_matters.words, vec![3]);
    // The first file sequence is `run`, new; a second import adds `walk` again by the same name index.
    let before = shape.names.len();
    shape.import_sequences(&dsq, &mut free).expect("imports");
    assert_eq!(shape.names.len(), before);
    assert_eq!(shape.sequences.len(), 5);
}

#[test]
fn a_refused_import_changes_nothing() {
    let mut shape = base_shape();
    let before = shape.clone();
    let mut dsq = parse(&sample(), &mut free).unwrap();
    dsq.node_names[1] = b"missing".to_vec();
    assert!(matches!(
        shape.import_sequences(&dsq, &mut free),
        Err(ShapeError::Bad(_))
    ));
    assert_eq!(shape, before);
    // A node twice in the file and once in the shape.
    let mut twice = parse(&sample(), &mut free).unwrap();
    twice.node_names[1] = b"ROOT".to_vec();
    assert!(shape.import_sequences(&twice, &mut free).is_err());
    assert_eq!(shape, before);
    // A budget that cannot pay.
    let dsq = parse(&sample(), &mut free).unwrap();
    assert_eq!(
        shape.import_sequences(&dsq, &mut |_| false),
        Err(ShapeError::OutOfBudget)
    );
    assert_eq!(shape, before);
}

#[test]
fn imports_into_one_shape_stop_at_the_sequence_cap() {
    let mut shape = base_shape();
    let mut dsq = parse(&sample(), &mut free).unwrap();
    let template = dsq.sequences[0].clone();
    dsq.sequences = vec![template; 100];
    let mut done = 0;
    let result = loop {
        match shape.import_sequences(&dsq, &mut free) {
            Ok(()) => done += 1,
            Err(e) => break e,
        }
        assert!(done < 100, "never stopped");
    };
    assert!(matches!(
        result,
        ShapeError::Cap {
            what: "sequences (with the imports)",
            ..
        }
    ));
    assert!(shape.sequences.len() <= MAX_SEQUENCES);
    assert_eq!(done, (MAX_SEQUENCES - 1) / 100);
}

#[test]
fn a_node_the_sets_cannot_hold_is_refused() {
    let mut shape = base_shape();
    for _ in 2..200 {
        let mut node = shape.nodes[1];
        node.name = 1;
        node.parent = 0;
        shape.nodes.push(node);
    }
    let mut dsq = parse(&sample(), &mut free).unwrap();
    // The file's n-th `arm` node maps to the shape's n-th, which is at index n (node 0 is the root).
    dsq.node_names = vec![b"arm".to_vec(); 199];
    dsq.sequences[0].sequence.rotation_matters = IntSet {
        first: 0,
        words: vec![0, 0, 0, 0, 0, 1 << 31],
    };
    let before = shape.clone();
    // File node 191 (the last bit a set holds) maps to the shape's node 192, which no set can hold.
    assert!(shape.import_sequences(&dsq, &mut free).is_err());
    assert_eq!(shape, before);
}

/// A hostile input: 16,384 names of 255 bytes sharing a 250-byte prefix, 4,096 nodes named the last one, and a file of
/// 4,000 node names equal to it and no sequence. Looked up by a scan per name this walks billions of bytes for a few million
/// units; the work must be a hash per name, and the charge must cover the bytes folded.
fn long_names_shape() -> (Shape, Dsq) {
    let mut shape = Shape::empty();
    let name = |i: usize| {
        let mut n = vec![b'a'; 250];
        n.extend_from_slice(format!("{i:05}").as_bytes());
        n
    };
    shape.names = (0..16_384).map(name).collect();
    let node = crate::shape::Node {
        name: 16_383,
        parent: -1,
        first_object: -1,
        first_child: -1,
        next_sibling: -1,
    };
    shape.nodes = vec![node; 4_096];
    let mut file = parse(&sample(), &mut free).unwrap();
    file.node_names = vec![name(16_383); 4_000];
    file.sequences.clear();
    (shape, file)
}

#[test]
fn name_lookups_are_hashed_and_charged_by_the_bytes_they_fold() {
    let (mut shape, file) = long_names_shape();
    let before = shape.clone();
    // A small budget refuses it before any work, and changes nothing.
    let mut left = 1_000_000u64;
    let refused = shape.import_sequences(&file, &mut |u| {
        if u > left {
            return false;
        }
        left -= u;
        true
    });
    assert_eq!(refused, Err(ShapeError::OutOfBudget));
    assert_eq!(shape, before);
    // Paid in full: the charge covers the name bytes (16,384 x 255 of the shape's, 4,000 x 255 of the file's) and the work is
    // a hash per name, far under a scan per name.
    let mut units = 0u64;
    let start = std::time::Instant::now();
    shape
        .import_sequences(&file, &mut |u| {
            units += u;
            true
        })
        .expect("imports");
    assert!(units >= 16_384 * 255 + 4_000 * 255, "{units}");
    assert!(
        start.elapsed() < std::time::Duration::from_millis(400),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn an_import_that_overflows_or_reaches_past_the_keyframes_is_refused() {
    let file = parse(&sample(), &mut free).unwrap();
    for what in 0..4 {
        let mut shape = base_shape();
        let before = shape.clone();
        let mut dsq = file.clone();
        match what {
            0 => dsq.sequences[0].sequence.base_rotation = i32::MAX,
            // A trigger and a ground frame that are read (an unread one's base is never used, `shape::fits`).
            1 => {
                dsq.sequences[0].sequence.first_trigger = i32::MAX;
                dsq.sequences[0].sequence.num_triggers = 1;
            }
            2 => {
                dsq.sequences[0].sequence.first_ground_frame = i32::MAX;
                dsq.sequences[0].sequence.num_ground_frames = 1;
            }
            // Two keyframes of a node that moves, with only one rotation in the file.
            _ => {
                dsq.sequences[0].sequence.num_keyframes = 2;
                dsq.sequences[0].sequence.rotation_matters = IntSet {
                    first: 0,
                    words: vec![1],
                };
            }
        }
        assert!(
            matches!(
                shape.import_sequences(&dsq, &mut free),
                Err(ShapeError::Bad(_))
            ),
            "case {what}"
        );
        assert_eq!(shape, before, "case {what}");
    }
}
