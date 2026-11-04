use super::*;

#[test]
fn a_heightfield_wraps_and_alternates_its_cut() {
    let mut heights = vec![0.0; HEIGHTFIELD_SAMPLES];
    heights[1 + HEIGHTFIELD_SIDE] = 2.5;
    let f = Heightfield {
        heights,
        layers: vec![Layer {
            material: "grass".into(),
            weights: vec![255; HEIGHTFIELD_SAMPLES],
        }],
        split: Split::Alternating,
    };
    assert_eq!(f.height(1 + HEIGHTFIELD_SIDE, 1), 2.5);
    assert_eq!(f.weight(0, 7), 255);
    assert_eq!(f.weight(1, 7), 0);
    assert!(f.main_diagonal(0, 0) && !f.main_diagonal(1, 0) && f.main_diagonal(1, 1));
}

#[test]
fn a_placement_holds_its_square_and_height_to_the_caps() {
    let p = HeightfieldPlacement::new([1.0, 2.0], f32::NAN, 1 << 20, true, vec![true; 3]);
    assert_eq!(p.origin, [1.0, 2.0, 0.0]);
    assert_eq!(p.square, MAX_SQUARE_SIZE as f32);
    assert_eq!(p.holes.len(), HEIGHTFIELD_SAMPLES);
    assert!(p.holes[2] && !p.holes[3]);
    assert_eq!(
        HeightfieldPlacement::new([0.0; 2], 1.0, -5, false, Vec::new()).square,
        1.0
    );
}

#[test]
fn contents_combine() {
    let c = Contents(Contents::SOLID.0 | Contents::LADDER.0);
    assert!(c.has(Contents::SOLID) && c.has(Contents::LADDER) && !c.has(Contents::WATER));
}
