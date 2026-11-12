use super::*;
use crate::hd::oct_decode;
use crate::hd::origin::Shift;

#[test]
fn a_sphere_is_closed_round_and_its_normals_point_out() {
    let d = sphere([1.0, 2.0, 3.0], 2.5, 8, 12, 200);
    assert_eq!(d.vertices.len(), 9 * 13);
    assert_eq!(d.indices.len(), 8 * 12 * 6);
    assert!(d.indices.iter().all(|&i| (i as usize) < d.vertices.len()));
    for v in &d.vertices {
        let r = [0, 1, 2].map(|k| v.position[k] - [1.0, 2.0, 3.0][k]);
        let len = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
        assert!((len - 2.5).abs() < 1e-4);
        let n = oct_decode(v.normal);
        let along = (n[0] * r[0] + n[1] * r[1] + n[2] * r[2]) / len;
        assert!(along > 0.999, "{along}");
        assert_eq!(v.color[3], 200);
    }
    let s = HdDistorter::new(
        d,
        DistortionLook {
            strength: 0.2,
            ..DistortionLook::default()
        },
    );
    assert!((s.radius - 2.5).abs() < 1e-3);
    assert!(
        s.centre
            .iter()
            .zip([1.0, 2.0, 3.0])
            .all(|(a, b)| (a - b).abs() < 1e-4)
    );
    assert!(s.is_drawn());
}

#[test]
fn a_look_out_of_range_is_held_and_one_that_does_nothing_is_not_drawn() {
    let wild = DistortionLook {
        strength: 1e9,
        edge: f32::NAN,
        map: None,
        map_scale: 0.0,
        scroll: [f32::INFINITY, -1e6],
        tint: [2.0, -1.0, f32::NAN, 0.5],
        chroma: 7.0,
    }
    .sanitize();
    assert_eq!(wild.strength, MAX_STRENGTH);
    assert_eq!(wild.edge, 0.0);
    assert_eq!(wild.map_scale, MAP_SCALE_RANGE.0);
    assert_eq!(wild.scroll, [0.0, -MAX_SCROLL]);
    assert_eq!(wild.tint, [1.0, 0.0, 1.0, 0.5]);
    assert_eq!(wild.chroma, 1.0);
    let nothing = HdDistorter::new(sphere([0.0; 3], 1.0, 4, 4, 255), DistortionLook::default());
    assert!(!nothing.is_drawn());
}

#[test]
fn a_distorter_moves_with_the_render_origin() {
    let mut s = HdDistorter::new(
        sphere([10.0, 0.0, 0.0], 1.0, 4, 6, 255),
        DistortionLook {
            strength: 0.1,
            ..DistortionLook::default()
        },
    );
    let before = s.draw.vertices[5].position;
    s.shift([-10.0, 0.0, 0.0]);
    assert!((s.centre[0]).abs() < 1e-5);
    assert!((s.draw.vertices[5].position[0] - (before[0] - 10.0)).abs() < 1e-5);
}

#[test]
fn the_budget_takes_what_fits_and_the_order_is_the_nearest_surface_first() {
    let mut b = DistortionBudget::default();
    assert!(b.take(MAX_DISTORTERS - 1, MAX_VERTICES - 10, 0));
    assert!(!b.take(1, 11, 0), "past the vertices");
    assert!(!b.take(1, 0, MAX_INDICES + 1), "past the indices");
    assert!(b.take(1, 10, 3));
    assert!(b.is_spent());
    assert!(!b.take(1, 0, 0), "past the count");
    // A big sphere's near side counts, not its centre.
    let order = nearest_first(
        [0.0; 3],
        [
            ([10.0, 0.0, 0.0], 1.0),
            ([20.0, 0.0, 0.0], 15.0),
            ([3.0, 0.0, 0.0], 0.0),
        ]
        .into_iter(),
    );
    assert_eq!(order, vec![2, 1, 0]);
}
