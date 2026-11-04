use super::*;
use crate::synth::{DifSpec, dif};

fn specs() -> Vec<DifSpec> {
    let mut out = Vec::new();
    for (version, plain_surfaces) in [
        (0, true),
        (0, false),
        (3, false),
        (5, false),
        (10, false),
        (11, false),
        (12, false),
        (13, false),
        (14, false),
    ] {
        for (everything, border) in [(false, 0), (true, 1), (false, 2), (true, 2)] {
            out.push(DifSpec {
                version,
                plain_surfaces,
                everything,
                border,
            });
        }
    }
    out
}

#[test]
fn every_synthetic_interior_reads_to_its_end_and_passes_the_check() {
    for s in specs() {
        let d = parse_with_border_pad(&dif(s)).unwrap_or_else(|e| panic!("{s:?}: {e}"));
        assert_eq!(d.trailing, 0, "{s:?}");
        assert_eq!(d.border_pad, s.border == 2, "{s:?}");
        // TGE's layout reads the file without the pad word to its end, and not one with it.
        let plain = parse(&dif(s));
        assert_eq!(
            plain
                .as_ref()
                .is_ok_and(|p| p.trailing == 0 && !p.border_pad),
            s.border != 2,
            "{s:?}"
        );
        let i = &d.details[0];
        assert_eq!(i.version, s.version);
        assert_eq!(i.plain_surfaces, s.plain_surfaces, "{s:?}");
        i.check().unwrap();
        assert_eq!(i.surfaces.len(), 6);
        assert_eq!(i.triangle_count(), 12);
        assert_eq!(i.materials, ["wall", "NULL"]);
        assert_eq!(i.surfaces[0].plane & PLANE_FLIPPED, PLANE_FLIPPED);
        assert_eq!(i.surfaces[0].lightmap_texgen.x[0], 0.125);
        assert_eq!(i.bsp_nodes[0].front, BspChild::Empty(0));
        assert_eq!(i.bsp_nodes[0].back, BspChild::Solid(0));
        assert_eq!(
            i.lightmaps[0].direction.is_some(),
            !s.plain_surfaces && (s.version == 0 || s.version == 5 || s.version >= 12)
        );
        assert_eq!(i.lightmap_border, (s.border == 1).then_some(2));
        if s.everything {
            assert!(d.preview.is_some());
            assert_eq!(d.sub_objects.len(), 1);
            assert_eq!(
                d.triggers[0].dict,
                [("key".to_string(), "value".to_string())]
            );
            assert_eq!(d.path_followers[0].way_points.len(), 1);
            assert_eq!(d.force_fields[0].color, [255, 0, 0, 128]);
            assert_eq!(d.ai_nodes[0].pos, [1.0, 2.0, 3.0]);
            assert_eq!(d.game_entities[0].class, "Item");
            assert_eq!(i.vehicle.as_ref().map(|v| v.points.len()), Some(8));
            assert_eq!(i.mirrors.len(), 1);
            assert_eq!(i.animated_lights.len(), 1);
            assert_eq!(i.static_meshes.len(), (s.version >= 10) as usize);
            if let Some(m) = i.static_meshes.first() {
                assert_eq!(m.materials.as_ref().map(|m| m.names.len()), Some(1));
                assert_eq!(m.diffuse_bitmaps.len(), 2);
            }
        }
    }
}

#[test]
fn a_bad_reference_fails_the_check_and_a_bad_version_the_read() {
    let s = DifSpec {
        version: 0,
        plain_surfaces: true,
        everything: false,
        border: 0,
    };
    let mut d = parse(&dif(s)).unwrap();
    d.details[0].windings[3] = 8;
    assert!(matches!(
        d.details[0].check(),
        Err(MapError::Index {
            what: "winding point",
            ..
        })
    ));
    let mut bytes = dif(s);
    bytes[0] = 43;
    assert!(matches!(
        parse(&bytes),
        Err(MapError::Version {
            what: "resource",
            ..
        })
    ));
    let mut bytes = dif(s);
    bytes[9] = 15;
    assert!(matches!(
        parse(&bytes),
        Err(MapError::Version {
            what: "interior",
            ..
        })
    ));
    let h = header(&dif(s)).unwrap();
    assert_eq!(
        (h.resource_version, h.preview, h.details, h.interior_version),
        (44, false, 1, Some(0))
    );
}

#[test]
fn every_cut_of_a_synthetic_file_is_refused_without_a_panic() {
    let bytes = dif(DifSpec {
        version: 14,
        plain_surfaces: false,
        everything: true,
        border: 1,
    });
    for n in 0..bytes.len() {
        assert!(parse(&bytes[..n]).is_err(), "cut at {n}");
    }
}
