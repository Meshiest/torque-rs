use torque_maps::synth::{DifSpec, dif_hulls};

use super::*;

const SPEC: DifSpec = DifSpec {
    version: 0,
    plain_surfaces: true,
    everything: false,
    border: 0,
};

/// What the file makes read the way it was before the files were kept: one reading for the hulls, one for the box.
fn read_afresh(bytes: &[u8]) -> Option<(u64, KeptInterior)> {
    let set = torque_mapcore::Importer::convexes(&torque_maps::legacy::IMPORTER, bytes).ok()?;
    let b = torque_maps::dif::parse(bytes).ok()?.details.first()?.bounds;
    Some((
        collide::map::interior::build_cost(&set),
        KeptInterior {
            bounds: Box3::new(b.min, b.max),
            geom: Arc::new(InteriorGeom::new(&set)),
            crc: torque_crc(bytes),
        },
    ))
}

#[test]
fn a_file_read_again_makes_the_same_interior_and_costs_the_same() {
    // Hulls of their own: no other test's file.
    let bytes = dif_hulls(SPEC, 5, 7);
    let (cost, fresh) = read_afresh(&bytes).unwrap();
    for _ in 0..2 {
        let mut asked = Vec::new();
        let kept = read_interior(&bytes, &mut |u| {
            asked.push(u);
            true
        })
        .unwrap();
        assert_eq!(asked, [cost]);
        assert_eq!(kept.crc, fresh.crc);
        assert_eq!(kept.bounds, fresh.bounds);
        assert_eq!(kept.geom.hulls.len(), fresh.geom.hulls.len());
        for (a, b) in kept.geom.hulls.iter().zip(&fresh.geom.hulls) {
            assert_eq!(a.points, b.points);
            assert_eq!(a.planes, b.planes);
        }
    }
    // A refused cost makes nothing, kept or not.
    assert!(read_interior(&bytes, &mut |_| false).is_none());
    let mut other = dif_hulls(SPEC, 2, 6);
    assert!(read_interior(&other, &mut |_| false).is_none());
    assert!(read_interior(&other, &mut |_| true).is_some());
    // A file that does not read is refused each time, and not kept.
    other.truncate(other.len() / 2);
    assert!(read_interior(&other, &mut |_| true).is_none());
    assert!(made::find(&other, torque_crc(&other)).is_none());
    assert!(read_interior(&other, &mut |_| true).is_none());
}

#[test]
fn the_kept_files_stay_bounded_and_a_new_one_is_kept_when_full() {
    let bytes = dif_hulls(SPEC, 3, 6);
    let kept = read_interior(&bytes, &mut |_| true).unwrap();
    // Stand-in files (the keeping reads no bytes): more than fit.
    let file = |i: usize| format!("kept file {i}").into_bytes();
    for i in 0..made::MAX_FILES + 40 {
        let f = file(i);
        made::keep(&f, torque_crc(&f), i as u64, kept.clone());
        assert!(made::len() <= made::MAX_FILES);
    }
    let last = file(made::MAX_FILES + 39);
    assert_eq!(
        made::find(&last, torque_crc(&last)).map(|m| m.0),
        Some(made::MAX_FILES as u64 + 39)
    );
    let first = file(0);
    assert!(
        made::find(&first, torque_crc(&first)).is_none(),
        "the one longest unused is let go"
    );
    assert_eq!(made::len(), made::MAX_FILES);
}
