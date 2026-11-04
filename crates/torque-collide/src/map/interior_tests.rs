use super::*;
use torque_maps::synth::{DifSpec, dif_hulls};

const SPEC: DifSpec = DifSpec {
    version: 0,
    plain_surfaces: true,
    everything: false,
    border: 0,
};

fn detail(count: u32, planes: u32) -> ConvexSet {
    let d = torque_maps::dif::parse(&dif_hulls(SPEC, count, planes)).expect("the file reads");
    torque_maps::legacy::convexes(d.details.first().expect("a detail"))
}

#[test]
fn a_box_hull_is_made_and_one_naming_too_many_planes_is_refused_before_its_work() {
    let i = detail(1, 6);
    let h = hull_of(&i, 0).expect("the box");
    assert_eq!(h.faces.len(), 6);
    // Its six planes named over and over: the same box while they and the box's six are within the cap.
    let fits = MAX_HULL_PLANES as u32 - 6;
    let i = detail(1, fits);
    assert_eq!(hull_of(&i, 0), Some(h));
    let i = detail(1, fits + 1);
    assert_eq!(hull_of(&i, 0), None);
    assert!(hull_cost(&i, 0) > u64::from(fits));
}

#[test]
fn hostile_hull_plane_ranges_are_refused_and_charged_for_their_whole_length() {
    // Sixteen hulls each naming 30,000 distinct planes, every one with the box behind it (all kept): before, each was
    // compared with every plane kept so far (half a billion comparisons a hull) before their count was looked at.
    let named = 30_000u32;
    let mut i = detail(16, named);
    let base = i.planes.len();
    i.planes.extend((0..named).map(|j| {
        let a = j as f32 * 1e-4;
        torque_mapcore::collision::Plane {
            normal: [a.cos(), a.sin(), 0.25],
            d: -10.0 - j as f32 * 1e-3,
        }
    }));
    for (j, r) in i.plane_refs.iter_mut().enumerate() {
        r.index = (base + j % named as usize) as u32;
        r.flipped = false;
    }
    assert_eq!(i.plane_refs.len(), 16 * named as usize);
    let started = std::time::Instant::now();
    let g = InteriorGeom::new(&i);
    assert!(g.hulls.is_empty());
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    for k in 0..16 {
        assert!(hull_cost(&i, k) >= u64::from(named), "hull {k}");
    }
    // The cost of a refused hull is held to the cap's polytope plus the names it counts.
    let p = MAX_HULL_PLANES as u64 + 1;
    assert!(hull_cost(&i, 0) <= u64::from(named) + 8 * (p + 1) + p * p * p * p / 6 + 16);
    assert!(build_cost(&i) >= 16 * u64::from(named));
}
