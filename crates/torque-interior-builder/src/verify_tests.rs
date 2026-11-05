use super::*;

/// The share of the full counts a debug test run takes.
const SHARE: f64 = 0.004;

#[test]
fn the_generated_maps_pass_verification() {
    for seed in 0..20 {
        let brushes = generated(seed);
        let r = run(&brushes, &Options::default(), Counts::scaled(SHARE), seed).unwrap();
        assert!(r.clean(), "map {seed}: {}", r.line());
        assert!(
            r.surface_checked > 0 && r.hits > 0 && r.bsp_checked > 0,
            "map {seed}: {}",
            r.line()
        );
    }
}

#[test]
fn a_map_file_builds_and_passes_verification() {
    let text = "{\n\"classname\" \"worldspawn\"\n\
        {\n( -256 -256 -32 ) ( -256 -255 -32 ) ( -256 -256 -31 ) floor 0 0 0 1 1\n\
        ( -256 -256 -32 ) ( -256 -256 -31 ) ( -255 -256 -32 ) floor 0 0 0 1 1\n\
        ( -256 -256 -32 ) ( -255 -256 -32 ) ( -256 -255 -32 ) floor 0 0 0 1 1\n\
        ( 256 256 0 ) ( 256 257 0 ) ( 257 256 0 ) floor 0 0 0 1 1\n\
        ( 256 256 0 ) ( 257 256 0 ) ( 256 256 1 ) floor 0 0 0 1 1\n\
        ( 256 256 0 ) ( 256 256 1 ) ( 256 257 0 ) floor 0 0 0 1 1\n}\n\
        {\n( 0 0 0 ) ( 0 1 0 ) ( 0 0 1 ) crate [ 0 1 0 0 ] [ 0 0 -1 0 ] 0 0.5 0.5\n\
        ( 0 0 0 ) ( 0 0 1 ) ( 1 0 0 ) crate [ 1 0 0 0 ] [ 0 0 -1 0 ] 0 0.5 0.5\n\
        ( 0 0 0 ) ( 1 0 0 ) ( 0 1 0 ) crate [ 1 0 0 0 ] [ 0 -1 0 0 ] 0 0.5 0.5\n\
        ( 64 64 64 ) ( 64 65 64 ) ( 65 64 64 ) crate [ 1 0 0 0 ] [ 0 -1 0 0 ] 0 0.5 0.5\n\
        ( 64 64 64 ) ( 65 64 64 ) ( 64 64 65 ) crate [ 1 0 0 0 ] [ 0 0 -1 0 ] 0 0.5 0.5\n\
        ( 64 64 64 ) ( 64 64 65 ) ( 64 65 64 ) crate [ 0 1 0 0 ] [ 0 0 -1 0 ] 0 0.5 0.5\n}\n}\n";
    let m = crate::map::parse(text, &crate::map::MapOptions::default()).unwrap();
    assert_eq!(m.brushes.len(), 2);
    let r = run(&m.brushes, &Options::default(), Counts::scaled(SHARE), 3).unwrap();
    assert!(r.clean(), "{}", r.line());
    assert_eq!(r.hulls, 2);
}

#[test]
fn damage_to_any_part_is_caught() {
    let brushes = generated(1);
    let good = build(&brushes, &Options::default()).unwrap();
    let counts = Counts::scaled(SHARE);
    let r = check(&brushes, &good, counts, 1).unwrap();
    assert!(r.clean() && r.covered > 0, "{}", r.line());
    let damaged = |f: &dyn Fn(&mut Interior)| {
        let mut b = good.clone();
        f(&mut b.dif.details[0]);
        check(&brushes, &b, counts, 1).unwrap()
    };
    // A surface drawn twice.
    let r = damaged(&|i| {
        let s = i.surfaces[0];
        i.surfaces.push(s);
        i.normal_lightmaps.push(0);
        i.alarm_lightmaps.push(0);
    });
    assert!(r.surface_misses > 0, "{}", r.line());
    // Texture planes shifted.
    let r = damaged(&|i| i.texgens.iter_mut().for_each(|t| t.x[3] += 0.01));
    assert!(r.uv_mismatches > 0, "{}", r.line());
    // A hull's plane moved.
    let r = damaged(&|i| {
        let p = i.hulls.hull_plane_indices[0] & !PLANE_FLIPPED;
        i.planes[p as usize].d += 0.25;
    });
    assert!(r.ray_mismatches > 0, "{}", r.line());
    // A BSP leaf turned.
    let r = damaged(&|i| {
        for n in &mut i.bsp_nodes {
            if let BspChild::Solid(_) = n.back {
                n.back = BspChild::Empty(0);
            }
        }
    });
    assert!(r.bsp_mismatches > 0, "{}", r.line());
    // An emit string and a polygon list cut.
    let r = damaged(&|i| {
        i.hulls.emit_strings.iter_mut().for_each(|b| *b = 0);
        i.hulls.poly_list_strings.iter_mut().for_each(|b| *b = 0);
    });
    assert!(
        r.emit_differences > 0 && r.poly_list_differences > 0,
        "{}",
        r.line()
    );
}
