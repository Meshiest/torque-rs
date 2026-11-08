//! The physics reader over a game's own files: run only when `TORQUE_SOURCE_GAME_DIR` names game folders (separated by `;`, each
//! with a `gameinfo.txt`), and skipped otherwise. Nothing is written and no game content is kept or printed beyond counts.
//!
//! The oracles need no game run: every point inside every plane of its own hull and no hull empty; the hulls of models that keep
//! their points in the model's space lie inside the model's hull box (which settles the axes and the scale); the pieces of
//! ragdoll solids, taken from their bones' spaces, lie in the render mesh's bounds.

use std::collections::HashSet;
use std::path::Path;

use torque_engine::EngineBuilder;
use torque_engine::assets::Assets;

use crate::ValveFormatPlugins;
use crate::collision::{HullSource, ModelCollision};
use crate::files::SourceFiles;
use crate::hull::bounds;
use crate::mdl::Mdl;
use crate::phy::Phy;
use crate::vvd::Vvd;

fn mount() -> Option<SourceFiles> {
    let dirs = std::env::var("TORQUE_SOURCE_GAME_DIR").ok()?;
    let mut files = SourceFiles::new();
    for d in dirs.split(';').filter(|d| !d.trim().is_empty()) {
        files.add_gameinfo(Path::new(d.trim())).ok()?;
    }
    Some(files)
}

#[derive(Default, Debug)]
struct Stats {
    models: usize,
    phy_read: usize,
    phy_refused: usize,
    phy_empty_files: usize,
    solids: usize,
    unread_solids: usize,
    pieces: usize,
    boxed_pieces: usize,
    most_planes: usize,
    from_phy: usize,
    from_box: usize,
    from_render: usize,
    no_hull: usize,
    hull_failures: usize,
    box_checked: usize,
    box_inside: usize,
    ragdolls: usize,
    ragdolls_in_mesh: usize,
    slowest_ms: f32,
}

#[test]
fn a_games_physics_files_read_and_pass_the_oracles() {
    let Some(files) = mount() else {
        eprintln!("TORQUE_SOURCE_GAME_DIR is not set: skipped");
        return;
    };
    let engine = EngineBuilder::new()
        .add_plugins(ValveFormatPlugins)
        .build()
        .unwrap();
    let assets = Assets::new(&engine);
    let mut st = Stats::default();

    // Every model with a physics file, and the first 300 without one (the fallbacks).
    let phys: HashSet<String> = files.list("models/", ".phy").into_iter().collect();
    let mut paths: Vec<String> = Vec::new();
    let mut without = 0;
    for m in files.list("models/", ".mdl") {
        let has = phys.contains(&format!("{}.phy", m.trim_end_matches(".mdl")));
        if has {
            paths.push(m);
        } else if without < 300 {
            without += 1;
            paths.push(m);
        }
    }
    for path in &paths {
        let started = std::time::Instant::now();
        let Ok(c) = ModelCollision::load(&assets, &files, path) else {
            continue;
        };
        st.slowest_ms = st.slowest_ms.max(started.elapsed().as_secs_f32() * 1000.0);
        st.models += 1;
        match c.source {
            HullSource::Phy => st.from_phy += 1,
            HullSource::MdlBox => st.from_box += 1,
            HullSource::RenderHull => st.from_render += 1,
        }
        if c.hulls.is_empty() {
            st.no_hull += 1;
        }
        let phy_path = format!("{}.phy", path.trim_end_matches(".mdl"));
        if !phys.contains(&phy_path) {
            continue;
        }
        let Ok(phy) = assets.load::<Phy>(&files, &phy_path) else {
            // A file with no bytes at all has no header to read: refused, and the fallbacks serve the model.
            if files.len_of(&phy_path).is_some_and(|n| n < 16) {
                st.phy_empty_files += 1;
            } else {
                st.phy_refused += 1;
            }
            continue;
        };
        st.phy_read += 1;
        st.solids += phy.solids.len();
        st.unread_solids += phy.solids.iter().filter(|s| s.ledges.is_empty()).count();
        for s in &phy.solids {
            for l in &s.ledges {
                st.pieces += 1;
                st.boxed_pieces += usize::from(l.boxed);
                st.most_planes = st.most_planes.max(l.hull.planes.len());
                let ok =
                    !l.hull.planes.is_empty() && l.points.iter().all(|p| l.hull.contains(*p, 1e-3));
                if !ok {
                    st.hull_failures += 1;
                }
            }
        }
        let Ok(mdl) = assets.load::<Mdl>(&files, path) else {
            continue;
        };
        let named = (0..phy.solids.len()).any(|i| {
            phy.props(i).is_some_and(|p| {
                !p.name.is_empty()
                    && mdl
                        .bones
                        .iter()
                        .any(|b| b.name.eq_ignore_ascii_case(&p.name))
            })
        });
        if c.source != HullSource::Phy {
            continue;
        }
        let pts: Vec<[f32; 3]> = c
            .hulls
            .iter()
            .flat_map(|h| h.points.iter().copied())
            .collect();
        let (lo, hi) = bounds(&pts);
        if !named {
            // The model's space already: inside the model's hull box (a little slack for the box's own rounding).
            let extent = (0..3)
                .map(|k| mdl.hull[1][k] - mdl.hull[0][k])
                .fold(0.0f32, f32::max);
            if extent > 1.0 {
                st.box_checked += 1;
                let slack = 0.5 + 0.02 * extent;
                if (0..3)
                    .all(|k| lo[k] >= mdl.hull[0][k] - slack && hi[k] <= mdl.hull[1][k] + slack)
                {
                    st.box_inside += 1;
                }
            }
        } else if phy.solids.len() > 2 {
            // A ragdoll: from the bones' spaces into the render mesh's bounds.
            let stem = path.trim_end_matches(".mdl");
            let Ok(vvd) = assets.load::<Vvd>(&files, &format!("{stem}.vvd")) else {
                continue;
            };
            let verts: Vec<[f32; 3]> = vvd.vertices.iter().map(|v| v.position).collect();
            if verts.len() < 4 {
                continue;
            }
            st.ragdolls += 1;
            let (vlo, vhi) = bounds(&verts);
            let extent = (0..3).map(|k| vhi[k] - vlo[k]).fold(1.0f32, f32::max);
            let outside: f32 = (0..3)
                .map(|k| (vlo[k] - lo[k]).max(0.0) + (hi[k] - vhi[k]).max(0.0))
                .sum();
            if outside <= 0.25 * extent {
                st.ragdolls_in_mesh += 1;
            }
        }
    }
    println!("physics files over a game's folders: {st:#?}");
    assert!(st.models > 0);
    // Reads: nothing refused, every hull passes the plane oracle and none is empty.
    assert_eq!(st.phy_refused, 0, "{st:?}");
    assert_eq!(st.hull_failures, 0, "{st:?}");
    // The axes and the scale: nearly every model keeping its points in the model's space lies inside its hull box.
    if st.box_checked >= 50 {
        assert!(
            st.box_inside * 100 >= st.box_checked * 90,
            "only {} of {} inside the hull box: {st:?}",
            st.box_inside,
            st.box_checked
        );
    }
    if st.ragdolls >= 20 {
        assert!(st.ragdolls_in_mesh * 100 >= st.ragdolls * 90, "{st:?}");
    }
}

/// A few real maps' static props placed as collision: counts of props placed and of hulls the collision library takes.
#[cfg(feature = "collide")]
#[test]
fn real_maps_static_props_become_collision() {
    use crate::bsp::Bsp;
    use crate::props::static_prop_convexes;
    use std::rc::Rc;
    use torque_collide::map::interior::InteriorGeom;

    let Some(files) = mount() else {
        eprintln!("TORQUE_SOURCE_GAME_DIR is not set: skipped");
        return;
    };
    let engine = EngineBuilder::new()
        .add_plugins(ValveFormatPlugins)
        .build()
        .unwrap();
    let assets = Assets::new(&engine);
    let mut maps = 0;
    for path in files.list("maps/", ".bsp") {
        if maps == 4 {
            break;
        }
        let Ok(map) = assets.load::<Bsp>(&files, &path) else {
            continue;
        };
        let Ok(props) = map.static_props() else {
            continue;
        };
        if props.len() < 20 {
            continue;
        }
        maps += 1;
        let mut collision = |m: &str| ModelCollision::load(&assets, &files, m).ok().map(Rc::new);
        let out = static_prop_convexes(&props, &mut collision);
        let geom = InteriorGeom::new(&out.set);
        println!(
            "{path}: {} props, {} placed, {} not solid, {} without a model, {} hulls placed, {} taken by the collision library, {} left out",
            props.len(),
            out.placed,
            out.not_solid,
            out.no_model,
            out.set.hulls.len(),
            geom.hulls.len(),
            out.left_out
        );
        assert_eq!(out.placed + out.not_solid + out.no_model, props.len());
        assert!(out.left_out == 0);
    }
}
