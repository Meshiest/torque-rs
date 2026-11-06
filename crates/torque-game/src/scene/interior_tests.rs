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
    // A file that does not read is refused again without being read.
    other.truncate(other.len() / 2);
    assert!(read_interior(&other, &mut |_| true).is_none());
    assert!(read_interior(&other, &mut |_| true).is_none());
}
