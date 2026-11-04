use super::*;
use crate::synth;

#[test]
fn a_synthetic_terrain_reads_back() {
    for version in 1..=NEWEST_VERSION {
        let bytes = synth::ter(version, &["grass", "", "rock"]);
        let t = parse(&bytes).unwrap();
        assert_eq!(t.version, version);
        assert_eq!(t.trailing, 0);
        assert_eq!(t.materials.len(), 2);
        assert_eq!(t.materials[1].slot, 2);
        assert_eq!(t.height(3, 0), 3.0 / HEIGHT_PER_METRE);
        assert_eq!(t.edge_terrains.is_some(), version >= 5);
        assert!(t.dominant(5) < 2);
    }
}

#[test]
fn empty_square_runs_go_along_x_and_stop_at_the_row_end() {
    // The first two runs of a typical map's list, then a run of 3 at (84, 85) and one past the row's end.
    let e = empty_squares(&[66816, 71763, 218452, 254 | 7 << 8 | 5 << 16]);
    assert!(e[5 * SIZE] && e[83 + 24 * SIZE]);
    assert!(e[84 + 85 * SIZE] && e[86 + 85 * SIZE] && !e[87 + 85 * SIZE]);
    assert!(e[255 + 7 * SIZE] && !e[8 * SIZE]);
    assert_eq!(e.iter().filter(|&&b| b).count(), 2 + 3 + 2);
    assert_eq!(block_origin(8.0), [-1024.0, -1024.0]);
    assert!(split45(0, 0) && !split45(1, 0) && split45(1, 1));
}

#[test]
fn versions_outside_the_legacy_ones_are_refused() {
    assert!(matches!(parse(&[0]), Err(MapError::Version { .. })));
    assert!(matches!(parse(&[7]), Err(MapError::Version { .. })));
    assert!(matches!(parse(&[3, 1, 2]), Err(MapError::Count { .. })));
}

#[test]
fn a_version_1_base_slot_names_the_compacted_materials() {
    // "grass", an empty name, "rock": the named materials are grass (0) and rock (1), whatever their slots (0 and 2).
    let mut t = parse(&synth::ter(1, &["grass", "", "rock"])).unwrap();
    t.material_map[0] = 1;
    t.material_map[1] = 2;
    assert_eq!((t.alpha(0, 0), t.alpha(1, 0)), (0, 255));
    // Base slot 2 names no material of the two.
    assert_eq!((t.alpha(0, 1), t.alpha(1, 1)), (0, 0));
}
