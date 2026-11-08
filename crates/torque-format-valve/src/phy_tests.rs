//! Physics models from synthetic files: the pieces and their hulls, the oracles that need no game, hostile files, the loader and
//! the fallbacks.

#[path = "phy_fuzz.rs"]
mod fuzz;
#[path = "phy_real_tests.rs"]
mod real;
#[path = "phy_synth.rs"]
pub mod synth;

use torque_engine::EngineBuilder;
use torque_engine::assets::Assets;

use crate::collision::{HullSource, ModelCollision, model_space_hulls};
use crate::files::SourceFiles;
use crate::hull::{Hull, bounds};
use crate::mdl::Mdl;
use crate::phy::{self, Phy, SolidKind};
use crate::{ValveFormatPlugins, synth as models};

/// A model header file whose hull box is `lo`..`hi`.
fn mdl_with_hull(lo: [f32; 3], hi: [f32; 3]) -> Vec<u8> {
    let mut b = models::mdl(8);
    for k in 0..3 {
        b[104 + k * 4..108 + k * 4].copy_from_slice(&lo[k].to_le_bytes());
        b[116 + k * 4..120 + k * 4].copy_from_slice(&hi[k].to_le_bytes());
    }
    b
}

/// Every point of every ledge is inside its own hull's planes and no hull is empty: the oracle for a file's pieces.
pub fn check_hulls(phy: &Phy, tol: f32) {
    for s in &phy.solids {
        for l in &s.ledges {
            assert!(!l.hull.planes.is_empty() && !l.hull.points.is_empty());
            for p in &l.points {
                assert!(l.hull.contains(*p, tol), "{p:?} outside its hull");
            }
        }
    }
}

#[test]
fn a_box_and_a_wedge_read_as_two_pieces() {
    let phy = Phy::parse(&synth::two_pieces()).unwrap();
    assert_eq!(phy.solids.len(), 1);
    let s = &phy.solids[0];
    assert_eq!(s.kind, SolidKind::Poly);
    assert!(!s.legacy);
    assert_eq!(s.ledges.len(), 2);
    assert_eq!(s.drag_axis_areas, [0.5, 1.0, 1.5]);
    check_hulls(&phy, 1e-3);
    // The box: six planes, its bounds as built (model inches).
    let b = &s.ledges[0];
    assert!(!b.boxed);
    assert_eq!(b.hull.planes.len(), 6);
    let (lo, hi) = b.hull.bounds();
    for k in 0..3 {
        assert!((lo[k] - [-8.0, -8.0, 0.0][k]).abs() < 1e-3, "{lo:?}");
        assert!((hi[k] - [8.0, 8.0, 16.0][k]).abs() < 1e-3, "{hi:?}");
    }
    // The wedge's six corners make five planes.
    assert_eq!(s.ledges[1].hull.planes.len(), 5);
    assert_eq!(phy.hulls().count(), 2);
    assert!(phy.notes.is_empty(), "{:?}", phy.notes);
    assert_eq!(phy.checksum, models::CHECKSUM);
}

#[test]
fn a_hull_stays_inside_the_models_box_and_a_wrong_axis_does_not() {
    let phy = Phy::parse(&synth::two_pieces()).unwrap();
    let mdl = Mdl::parse(&mdl_with_hull([-8.5, -8.5, -0.5], [20.5, 10.5, 16.5])).unwrap();
    let hulls = model_space_hulls(&phy, &mdl);
    assert_eq!(hulls.len(), 2);
    let inside = |hulls: &[Hull]| {
        hulls.iter().all(|h| {
            let (lo, hi) = h.bounds();
            (0..3).all(|k| lo[k] >= mdl.hull[0][k] - 1e-3 && hi[k] <= mdl.hull[1][k] + 1e-3)
        })
    };
    assert!(inside(&hulls));
    // The same points read with the axes the other way about, or the scale wrong, leave the box.
    let swapped: Vec<Hull> = hulls
        .iter()
        .map(|h| h.mapped(&|p| [p[0], -p[2], p[1]]))
        .collect();
    assert!(!inside(&swapped));
    let inches_as_meters: Vec<Hull> = hulls
        .iter()
        .map(|h| h.mapped(&|p| p.map(|v| v * 39.37)))
        .collect();
    assert!(!inside(&inches_as_meters));
}

#[test]
fn the_older_layout_reads_the_same_pieces() {
    let pieces = vec![
        synth::box_points([-8.0, -8.0, 0.0], [8.0, 8.0, 16.0]),
        synth::wedge(),
    ];
    let new = Phy::parse(&synth::file(&[synth::modern(0, &pieces)], "")).unwrap();
    let old = Phy::parse(&synth::file(&[synth::legacy(&pieces)], "")).unwrap();
    assert!(old.solids[0].legacy);
    assert_eq!(new.solids[0].ledges, old.solids[0].ledges);
}

#[test]
fn the_text_gives_mass_damping_and_names() {
    let phy = Phy::parse(&synth::two_pieces()).unwrap();
    let p = phy.props(0).unwrap();
    assert_eq!(p.name, "crate");
    assert_eq!(p.surface_prop, "wood");
    assert_eq!((p.mass, p.damping, p.rot_damping), (50.5, 0.1, 0.2));
    assert_eq!((p.inertia, p.volume), (1.5, 4096.0));
    assert_eq!(phy.text.edit_params.len(), 3);
    assert!(phy.props(1).is_none());
}

#[test]
fn ragdoll_constraints_parse() {
    let phy = Phy::parse(&synth::ragdoll()).unwrap();
    assert_eq!(phy.solids.len(), 3);
    assert_eq!(phy.props(2).unwrap().name, "head");
    assert_eq!(phy.text.constraints.len(), 2);
    let c = &phy.text.constraints[0];
    assert_eq!((c.parent, c.child), (0, 1));
    assert_eq!(c.min, [-10.0, -5.0, -1.0]);
    assert_eq!(c.max, [10.0, 5.0, 1.0]);
    assert_eq!(c.friction, [1.0, 0.0, 0.0]);
    assert_eq!(
        phy.text.collision_rules[0],
        ("collisionpair".to_string(), "0,2".to_string())
    );
    for c in &phy.text.constraints {
        assert!(c.parent < phy.solids.len() && c.child < phy.solids.len());
    }
}

#[test]
fn indices_past_the_solids_are_dropped() {
    let text = "solid { \"index\" \"7\" \"name\" \"x\" }\nragdollconstraint { \"parent\" \"0\" \"child\" \"9\" }\nragdollconstraint { \"parent\" \"-1\" \"child\" \"0\" }\n";
    let pieces = vec![synth::box_points([0.0; 3], [4.0; 3])];
    let phy = Phy::parse(&synth::file(&[synth::modern(0, &pieces)], text)).unwrap();
    assert!(phy.text.solids.is_empty() && phy.text.constraints.is_empty());
}

#[test]
fn a_bone_named_solid_is_taken_to_model_space_by_its_bone() {
    // The synthetic model's one bone is "root"; a solid named for it is moved by the bone's rest pose.
    let mut model = models::mdl(8);
    let bone = 408;
    // pose_to_bone's x translation (the matrix is at 96 in the bone, row-major 3 x 4: the translation is its fourth entry).
    model[bone + 96 + 12..bone + 96 + 16].copy_from_slice(&(-5.0f32).to_le_bytes());
    for (k, v) in [(0usize, 1.0f32), (5, 1.0), (10, 1.0)] {
        model[bone + 96 + k * 4..bone + 100 + k * 4].copy_from_slice(&v.to_le_bytes());
    }
    let mdl = Mdl::parse(&model).unwrap();
    let text = "solid { \"index\" \"0\" \"name\" \"root\" }\n";
    assert_eq!(mdl.bones[0].name, "root");
    let pieces = vec![synth::box_points([0.0; 3], [4.0; 3])];
    let named = Phy::parse(&synth::file(&[synth::modern(0, &pieces)], text)).unwrap();
    let plain = Phy::parse(&synth::file(&[synth::modern(0, &pieces)], "")).unwrap();
    let a = model_space_hulls(&named, &mdl);
    let b = model_space_hulls(&plain, &mdl);
    let (alo, _) = a[0].bounds();
    let (blo, _) = b[0].bounds();
    // Bone space to model space is the inverse of pose-to-bone: +5 along x.
    assert!((alo[0] - blo[0] - 5.0).abs() < 1e-3, "{alo:?} {blo:?}");
    for p in &a[0].points {
        assert!(a[0].contains(*p, 1e-3));
    }
}

#[test]
fn other_kinds_are_noted_and_leave_no_pieces() {
    let pieces = vec![synth::box_points([0.0; 3], [4.0; 3])];
    let f = synth::file(
        &[
            synth::modern(1, &pieces),
            synth::modern(2, &[]),
            synth::modern(3, &[]),
            synth::modern(9, &[]),
            synth::modern(0, &pieces),
        ],
        "",
    );
    let phy = Phy::parse(&f).unwrap();
    assert_eq!(phy.solids.len(), 5);
    assert_eq!(phy.solids[0].kind, SolidKind::Mopp);
    assert_eq!(phy.solids[1].kind, SolidKind::Ball);
    assert_eq!(phy.solids[2].kind, SolidKind::Virtual);
    assert_eq!(phy.solids[3].kind, SolidKind::Other(9));
    assert_eq!(phy.hulls().count(), 1);
    assert_eq!(phy.notes.len(), 4);
}

#[test]
fn flat_pieces_are_dropped_and_few_points_still_hull() {
    let flat = vec![
        synth::stored([0.0, 0.0, 0.0]),
        synth::stored([4.0, 0.0, 0.0]),
        synth::stored([0.0, 4.0, 0.0]),
        synth::stored([4.0, 4.0, 0.0]),
    ];
    let tet = vec![
        synth::stored([0.0, 0.0, 0.0]),
        synth::stored([4.0, 0.0, 0.0]),
        synth::stored([0.0, 4.0, 0.0]),
        synth::stored([0.0, 0.0, 4.0]),
    ];
    let phy = Phy::parse(&synth::file(&[synth::modern(0, &[flat, tet])], "")).unwrap();
    assert_eq!(phy.solids[0].ledges.len(), 1);
    assert_eq!(phy.solids[0].ledges[0].hull.planes.len(), 4);
}

fn one_ledge_surface() -> Vec<u8> {
    synth::surface(&[synth::box_points([0.0; 3], [4.0; 3])])
}

fn solid_of(surface: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"VPHY");
    v.extend_from_slice(&0x100u16.to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes());
    v.extend_from_slice(&(surface.len() as i32).to_le_bytes());
    v.extend_from_slice(&[0u8; 16]);
    v.extend_from_slice(surface);
    v
}

#[test]
fn a_tree_that_names_one_node_twice_reads_it_once() {
    // Two leaves; the root's right child is made the left child (the leaf next to the root): one ledge, read once.
    let mut s = synth::surface(&[
        synth::box_points([0.0; 3], [4.0; 3]),
        synth::box_points([5.0; 3], [9.0; 3]),
    ]);
    s[48..52].copy_from_slice(&28i32.to_le_bytes());
    let phy = Phy::parse(&synth::file(&[solid_of(&s)], "")).unwrap();
    assert_eq!(phy.solids[0].ledges.len(), 1);
}

#[test]
fn a_node_that_is_its_own_child_ends() {
    // The root's right offset is -28 + 28: itself (offset 0 would make it a leaf, so use a node whose left child is itself by
    // pointing the right child at it from the leaf next door).
    let mut s = synth::surface(&[
        synth::box_points([0.0; 3], [4.0; 3]),
        synth::box_points([5.0; 3], [9.0; 3]),
    ]);
    // The left leaf (at 76) gets a right offset of -28: its parent, the root. Each is then visited once.
    s[76..80].copy_from_slice(&(-28i32).to_le_bytes());
    let phy = Phy::parse(&synth::file(&[solid_of(&s)], "")).unwrap();
    assert!(phy.solids[0].ledges.len() <= 2);
}

#[test]
fn a_tree_deeper_than_the_cap_is_refused_for_its_solid() {
    // A chain: each node's left child is the next node and its right child the leaf at the end.
    let depth = phy::MAX_TREE_DEPTH + 10;
    let mut s = vec![0u8; 48];
    s[32..36].copy_from_slice(&48i32.to_le_bytes());
    let leaf_at = 48 + depth * 28;
    for i in 0..depth {
        let at = 48 + i * 28;
        let mut node = [0u8; 28];
        node[..4].copy_from_slice(&((leaf_at - at) as i32).to_le_bytes());
        s.extend_from_slice(&node);
    }
    let mut leaf = [0u8; 28];
    leaf[4..8].copy_from_slice(&28i32.to_le_bytes());
    s.extend_from_slice(&leaf);
    s.extend_from_slice(&[0u8; 16]);
    let phy = Phy::parse(&synth::file(&[solid_of(&s)], "")).unwrap();
    assert!(phy.solids[0].ledges.is_empty());
    assert!(
        phy.notes.iter().any(|n| n.contains("too deep")),
        "{:?}",
        phy.notes
    );
}

#[test]
fn points_past_the_solid_leave_that_ledge_out() {
    let mut s = one_ledge_surface();
    // The ledge (after the 48-byte header and the one node) starts with its point offset.
    let ledge = 48 + 28;
    s[ledge..ledge + 4].copy_from_slice(&1_000_000i32.to_le_bytes());
    let phy = Phy::parse(&synth::file(&[solid_of(&s)], "")).unwrap();
    assert!(phy.solids[0].ledges.is_empty());
    s[ledge..ledge + 4].copy_from_slice(&(-1_000_000i32).to_le_bytes());
    let phy = Phy::parse(&synth::file(&[solid_of(&s)], "")).unwrap();
    assert!(phy.solids[0].ledges.is_empty());
}

#[test]
fn counts_over_the_caps_refuse_the_file() {
    let mut s = one_ledge_surface();
    let ledge = 48 + 28;
    s[ledge + 12..ledge + 14].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(matches!(
        Phy::parse(&synth::file(&[solid_of(&s)], "")),
        Err(crate::Error::TooLarge { .. })
    ));
    let mut f = synth::file(&[], "");
    f[8..12].copy_from_slice(&100_000i32.to_le_bytes());
    assert!(matches!(Phy::parse(&f), Err(crate::Error::TooLarge { .. })));
    let big = vec![0u8; phy::MAX_FILE_BYTES as usize + 1];
    assert!(matches!(
        Phy::parse(&big),
        Err(crate::Error::TooLarge { .. })
    ));
}

#[test]
fn a_header_that_misses_its_solids_is_an_error() {
    assert!(Phy::parse(&[]).is_err());
    assert!(Phy::parse(&[0u8; 16]).is_err());
    // Two solids promised, one there.
    let mut f = synth::two_pieces();
    f[8..12].copy_from_slice(&2i32.to_le_bytes());
    assert!(Phy::parse(&f).is_err());
}

#[test]
fn the_loader_gives_a_model_its_pieces_and_the_fallbacks_follow() {
    let engine = EngineBuilder::new()
        .add_plugins(ValveFormatPlugins)
        .build()
        .unwrap();
    let assets = Assets::new(&engine);
    let mut files = SourceFiles::new();
    files.mount_memory_first(
        "mem",
        [
            (
                "models/crate.mdl".to_string(),
                mdl_with_hull([-8.5, -8.5, -0.5], [20.5, 10.5, 16.5]),
            ),
            ("models/crate.phy".to_string(), synth::two_pieces()),
            (
                "models/box.mdl".to_string(),
                mdl_with_hull([-1.0, -2.0, 0.0], [1.0, 2.0, 6.0]),
            ),
            (
                "models/bad.mdl".to_string(),
                mdl_with_hull([0.0; 3], [2.0; 3]),
            ),
            // A physics file that is refused: a header of 1,000 solids it does not hold.
            (
                "models/bad.phy".to_string(),
                vec![16, 0, 0, 0, 0, 0, 0, 0, 0xe8, 3, 0, 0, 0, 0, 0, 0],
            ),
            ("models/flat.mdl".to_string(), models::mdl(8)),
            ("models/flat.vvd".to_string(), models::vvd()),
        ],
    );
    let c = ModelCollision::load(&assets, &files, "models/crate.mdl").unwrap();
    assert_eq!(c.source, HullSource::Phy);
    assert_eq!(c.hulls.len(), 2);
    assert!(c.bounds.is_some());
    // No physics file: the header's box.
    let b = ModelCollision::load(&assets, &files, "models/box.mdl").unwrap();
    assert_eq!(b.source, HullSource::MdlBox);
    assert_eq!(b.hulls.len(), 1);
    assert_eq!(b.hulls[0].bounds(), ([-1.0, -2.0, 0.0], [1.0, 2.0, 6.0]));
    assert!(
        b.notes.iter().any(|n| n.contains("box.phy")),
        "{:?}",
        b.notes
    );
    // A refused physics file: the box again, and the note says why.
    let bad = ModelCollision::load(&assets, &files, "models/bad.mdl").unwrap();
    assert_eq!(bad.source, HullSource::MdlBox);
    assert!(
        bad.notes.iter().any(|n| n.contains("bad.phy")),
        "{:?}",
        bad.notes
    );
    // No box and a three-vertex mesh: the render hull has no volume to give.
    let flat = ModelCollision::load(&assets, &files, "models/flat.mdl").unwrap();
    assert_eq!(flat.source, HullSource::RenderHull);
    assert!(flat.hulls.is_empty());
    assert!(ModelCollision::load(&assets, &files, "models/none.mdl").is_err());
}

#[test]
fn the_render_hull_fallback_hulls_the_meshs_points() {
    let mdl = Mdl::parse(&models::mdl(8)).unwrap();
    let cube: Vec<[f32; 3]> = Hull::of_box([-3.0; 3], [5.0; 3]).points;
    let mut asked = 0;
    let c = ModelCollision::from_parts(None, Some("no file".into()), &mdl, &mut || {
        asked += 1;
        Some(cube.clone())
    });
    assert_eq!(asked, 1);
    assert_eq!(c.source, HullSource::RenderHull);
    assert_eq!(c.hulls.len(), 1);
    assert_eq!(c.hulls[0].bounds(), bounds(&cube));
    assert!(c.bounds.is_some());
    // With a box in the header the mesh is not even asked for.
    let mdl = Mdl::parse(&mdl_with_hull([0.0; 3], [1.0; 3])).unwrap();
    let c = ModelCollision::from_parts(None, None, &mdl, &mut || unreachable!());
    assert_eq!(c.source, HullSource::MdlBox);
}

#[cfg(feature = "collide")]
#[test]
fn static_props_place_their_hulls_for_the_collision() {
    use crate::bsp::StaticProp;
    use crate::props::static_prop_convexes;
    use std::rc::Rc;
    use torque_collide::map::interior::InteriorGeom;

    let phy = Phy::parse(&synth::two_pieces()).unwrap();
    let mdl = Mdl::parse(&mdl_with_hull([-8.5, -8.5, -0.5], [20.5, 10.5, 16.5])).unwrap();
    let model = Rc::new(ModelCollision::from_parts(
        Some(&phy),
        None,
        &mdl,
        &mut || None,
    ));
    let prop = |solid: u8, origin: [f32; 3], yaw: f32| StaticProp {
        model: "models/crate.mdl".into(),
        origin,
        angles: [0.0, yaw, 0.0],
        skin: 0,
        solid,
        leaves: Vec::new(),
    };
    let props = vec![
        prop(6, [100.0, 0.0, 0.0], 0.0),
        prop(6, [0.0, 200.0, 0.0], 90.0),
        prop(2, [0.0, 0.0, 300.0], 0.0),
        prop(0, [9.0, 9.0, 9.0], 0.0),
        StaticProp {
            model: "models/missing.mdl".into(),
            ..prop(6, [0.0; 3], 0.0)
        },
    ];
    let mut asked = Vec::new();
    let out = static_prop_convexes(&props, &mut |m| {
        asked.push(m.to_string());
        (m == "models/crate.mdl").then(|| model.clone())
    });
    assert_eq!(asked, ["models/crate.mdl", "models/missing.mdl"]);
    assert_eq!((out.placed, out.not_solid, out.no_model), (3, 1, 1));
    // Two pieces for each of two solid props, and the bounding box once.
    assert_eq!(out.set.hulls.len(), 5);
    let geom = InteriorGeom::new(&out.set);
    assert_eq!(geom.hulls.len(), 5);
    // The first prop's box spans x 92..108; the turned prop's (a quarter turn) spans y 192..208.
    let (b0, b1) = (geom.hulls[0].bounds, geom.hulls[2].bounds);
    assert!((b0.min[0] - 92.0).abs() < 1e-2 && (b0.max[0] - 108.0).abs() < 1e-2);
    assert!((b1.min[1] - 192.0).abs() < 1e-2 && (b1.max[1] - 208.0).abs() < 1e-2);
    // The third prop's box is the model's bounding box.
    let b4 = geom.hulls[4].bounds;
    assert!((b4.min[2] - 299.5).abs() < 1e-2 && (b4.max[2] - 316.5).abs() < 1e-2);
}
