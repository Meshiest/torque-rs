use std::collections::HashMap;

use ghost_world::tge::TgeClientSet;
use ghost_world::world::ClientWorld;
use ghosts::shapes::StaticShape;
use ghosts::{F32, TgeClass, TgeRecord, TgeShapeBase, Xf};

use super::*;
use crate::world::tests::{datablock, ghost};

struct Files(HashMap<String, Vec<u8>>);

impl Assets for Files {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.get(&path.to_ascii_lowercase()).cloned()
    }
}

const CRATE: &str = "demo/data/shapes/crate.dts";

/// A world of one StaticShape (ghost 1) at `at`, its shape a solid box.
fn world(at: [f32; 3]) -> (ClientWorld<TgeClientSet>, Files) {
    let b = [-1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
    let dts = shapes::synth::solid_dts(b, &[("detail32", b)], [0, 0, 0, 32767], [0.0; 3], false);
    let files = Files([(CRATE.to_string(), dts)].into());
    let mut w = ClientWorld::<TgeClientSet>::default();
    w.datablocks.insert(
        3,
        datablock("StaticShapeData", &[("shapeFile", Value::str(CRATE))]),
    );
    let record = StaticShape {
        shape: TgeShapeBase {
            game: ghosts::parts::GameBase {
                scale: None,
                datablock: Some(3),
            },
            body: None,
        },
        transform: Some((
            Xf {
                position: at.map(F32::new),
                quat: [F32::new(0.0); 3],
                negative_w: false,
            },
            [F32::new(1.0); 3],
        )),
        powered: false,
    };
    w.ghosts.insert(
        1,
        ghost(TgeClass::StaticShape, TgeRecord::StaticShape(record)),
    );
    (w, files)
}

fn draws_of<H: Hooks<ClientWorld<TgeClientSet>>>(
    w: &ClientWorld<TgeClientSet>,
    files: &Files,
    hooks: &H,
    select: ShapeSelect,
    cull: Option<&dyn Cull>,
) -> (ShapeDraws, Vec<String>) {
    let mut textures = TextureSet::new(files);
    let mut notes = Vec::new();
    let out = draws_culled(
        w,
        hooks,
        files,
        &mut textures,
        &mut notes,
        // (The synthetic shapes carry no radius: the most detailed level.)
        DetailPolicy::Highest,
        select,
        cull,
    );
    (out, notes)
}

struct Nothing;

impl Cull for Nothing {
    fn keeps(&self, _: [f32; 3], _: f32) -> bool {
        false
    }
}

struct Lamp;

impl<W: ShapeWorld + ?Sized> Hooks<W> for Lamp {
    type Live = ();

    fn lights(&self, centre: [f32; 3], _: f32) -> Vec<PointLight> {
        vec![PointLight {
            position: [centre[0], centre[1], centre[2] + 2.0],
            diffuse: [1.0; 3],
            attenuation: [1.0, 0.0, 0.0],
        }]
    }
}

#[test]
fn a_static_shape_is_drawn_where_its_record_puts_it() {
    let (w, files) = world([10.0, 0.0, 0.0]);
    let (out, notes) = draws_of(&w, &files, &(), ShapeSelect::All, None);
    assert!(!out.opaque.is_empty(), "{notes:?}");
    assert_eq!(out.opaque_owners.len(), out.opaque.len());
    assert!(out.opaque_owners.iter().all(|o| *o == 1));
    assert_eq!(out.transforms[&1].column(3), [10.0, 0.0, 0.0]);
    assert!(
        out.opaque[0]
            .label
            .contains(&format!("StaticShape 1 {CRATE}"))
    );
    let xs: Vec<f32> = out
        .opaque
        .iter()
        .flat_map(|d| d.vertices.iter().map(|v| v.position[0]))
        .collect();
    assert!(xs.iter().all(|x| (8.9..=11.1).contains(x)), "{xs:?}");
    // No hooks: no point lights; no laps outside the control object's builds.
    assert!(out.opaque.iter().all(|d| d.lights.is_empty()));
    assert!(out.laps.is_none());
}

#[test]
fn a_shape_is_lit_by_the_lights_its_hooks_give() {
    let (w, files) = world([0.0; 3]);
    let (out, _) = draws_of(&w, &files, &Lamp, ShapeSelect::All, None);
    assert!(!out.opaque.is_empty());
    assert!(out.opaque.iter().all(|d| d.lights.len() == 1));
}

#[test]
fn a_cull_leaves_out_what_it_cannot_see_but_never_the_control_object() {
    let (mut w, files) = world([0.0; 3]);
    let (out, _) = draws_of(&w, &files, &(), ShapeSelect::All, Some(&Nothing));
    assert!(out.opaque.is_empty() && out.translucent.is_empty());
    assert_eq!(out.culled, 1);
    w.control.object = Some(1);
    let (out, _) = draws_of(&w, &files, &(), ShapeSelect::All, Some(&Nothing));
    assert!(!out.opaque.is_empty());
    assert_eq!(out.culled, 0);
}

#[test]
fn the_control_object_is_selected_apart_from_the_rest() {
    let (mut w, files) = world([0.0; 3]);
    assert!(!is_dynamic(&w, 1) && !is_moving(&w, 1) && !live_split(&w));
    let (only, _) = draws_of(&w, &files, &(), ShapeSelect::OnlyControl, None);
    assert!(only.opaque.is_empty());
    assert!(only.laps.is_some());
    let (rest, _) = draws_of(&w, &files, &(), ShapeSelect::NotControl, None);
    assert!(!rest.opaque.is_empty());
    w.control.object = Some(1);
    w.animate = true;
    assert!(is_dynamic(&w, 1) && live_split(&w));
    let (only, _) = draws_of(&w, &files, &(), ShapeSelect::OnlyControl, None);
    assert!(!only.opaque.is_empty());
    let (rest, _) = draws_of(&w, &files, &(), ShapeSelect::NotControl, None);
    assert!(rest.opaque.is_empty());
}

#[test]
fn a_shape_that_does_not_load_is_noted_and_not_drawn() {
    let (w, _) = world([0.0; 3]);
    let empty = Files(HashMap::new());
    let (out, notes) = draws_of(&w, &empty, &(), ShapeSelect::All, None);
    assert!(out.opaque.is_empty());
    assert!(notes.iter().any(|n| n.contains("not loaded")), "{notes:?}");
}

#[test]
fn a_shadow_only_body_and_a_view_model_are_told_by_their_labels() {
    let body = format!("shape {}", shadow_body_label("Player 4 a.dts object 0"));
    assert!(is_shadow_body(&body) && !is_image(&body));
    let image = format!(
        "shape {}",
        view_model_label(&format!("{IMAGE_LABEL}0 of Player 4 b.dts"))
    );
    assert!(is_image(&image) && is_view_model(&image) && !is_shadow_body(&image));
    assert!(!shadow_body());
    assert!(with_shadow_body(true, shadow_body));
}

#[test]
fn orient_from_dir_makes_a_right_handed_frame_about_the_direction() {
    let m = Mat {
        m: orient_from_dir([0.0, 1.0, 0.0]),
    };
    assert_eq!(m.column(1), [0.0, 1.0, 0.0]);
    assert_eq!(m.column(0), [1.0, 0.0, 0.0]);
    assert_eq!(m.column(2), [0.0, 0.0, 1.0]);
    // Straight up: the x axis falls back to (0, -1, 0).
    let up = Mat {
        m: orient_from_dir([0.0, 0.0, 1.0]),
    };
    assert_eq!(up.column(0), [0.0, -1.0, 0.0]);
}

#[test]
fn a_projectile_grown_by_its_render_transform_is_drawn_at_that_scale() {
    let mut w = ClientWorld::<TgeClientSet>::default();
    w.datablocks.insert(
        4,
        datablock(
            "ProjectileData",
            &[("projectileShapeName", Value::str(CRATE))],
        ),
    );
    let record = ghosts::projectile::Projectile {
        game: ghosts::parts::GameBase {
            scale: None,
            datablock: Some(4),
        },
        ..Default::default()
    };
    w.ghosts.insert(
        2,
        ghost(TgeClass::Projectile, TgeRecord::Projectile(record)),
    );
    let placed = |w: &ClientWorld<TgeClientSet>| {
        projectile_placement(w, 2, &w.ghosts[&2], [0.0; 3]).map(|(m, s, _)| (m, s))
    };
    assert_eq!(
        placed(&w),
        None,
        "a projectile the client does not run is not drawn"
    );
    let mut m = crate::images::scale_columns(&Mat::euler_z(0.5), [0.25; 3]);
    m.set_column3([1.0, 2.0, 3.0]);
    let look = ghost_world::world::ProjectileLook {
        birth: 0,
        transform: m.m,
        velocity: [0.0, 1.0, 0.0],
        fade: 1.0,
    };
    w.projectile_render.insert(2, look);
    let (at, scale) = placed(&w).expect("drawn");
    assert!(scale.iter().all(|s| (s - 0.25).abs() < 1e-6), "{scale:?}");
    let turn = Mat::euler_z(0.5);
    for k in 0..3 {
        let (a, b) = (at.column(k), turn.column(k));
        assert!(
            (0..3).all(|i| (a[i] - b[i]).abs() < 1e-6),
            "column {k}: {a:?}"
        );
    }
    assert_eq!(at.column(3), [1.0, 2.0, 3.0]);
    // An unscaled transform is taken as it is.
    let mut whole = Mat::euler_z(0.5);
    whole.set_column3([1.0, 2.0, 3.0]);
    w.projectile_render.insert(
        2,
        ghost_world::world::ProjectileLook {
            transform: whole.m,
            ..look
        },
    );
    assert_eq!(placed(&w), Some((whole, [1.0; 3])));
}
