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

// ---- plugin ghosts drawn as shapes

use ghosts::codec::{Codec, Layout, R};
use ghosts::plugin::{
    DatablockRef, DrawnShapes, FieldSpec, GhostRecord, NetField, NetRecord, PluginClass, Replicate,
    ShapeDraw, ShapeFn,
};
use ghosts::{ObjectView, PackCtx};

/// A plugin's record as the derive writes it: a pose, a datablock and a size.
#[derive(Debug, Clone, PartialEq, Default)]
struct Prop {
    pose: Xf,
    data: DatablockRef,
    size: f32,
}

impl Layout for Prop {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.pose.layout(c, "transform", FieldSpec::DEFAULT)?;
        self.data.layout(c, "dataBlock", FieldSpec::DEFAULT)?;
        self.size.layout(c, "size", FieldSpec::DEFAULT)
    }
}

impl NetRecord for Prop {
    fn merge(&mut self, sent: &Prop) {
        *self = sent.clone();
    }
    fn datablock(&self) -> Option<u32> {
        self.data.datablock()
    }
    fn transform(&self) -> Option<Xf> {
        Some(self.pose)
    }
    fn script_fields(&self) -> Vec<(String, u32, Value)> {
        Vec::new()
    }
}

impl GhostRecord for Prop {
    fn pack(_: &dyn ObjectView, _: u32, _: &PackCtx) -> (Prop, u32) {
        (Prop::default(), 0)
    }
}

fn pose(at: [f32; 3], yaw: f32) -> Xf {
    let half = yaw * 0.5;
    Xf {
        position: at.map(F32::new),
        quat: [F32::new(0.0), F32::new(0.0), F32::new(half.sin())],
        negative_w: false,
    }
}

fn prop_class(replicate: Replicate) -> PluginClass {
    PluginClass::ghost::<Prop>("Prop", replicate)
}

/// The class drawn as its datablock's `model` field says, scaled by the record's size.
fn drawn_props(class: PluginClass) -> DrawnShapes {
    let mut drawn = DrawnShapes::default();
    drawn.insert(
        class,
        ShapeFn::new::<Prop>(|prop, blocks| {
            let model = blocks.text(prop.data.datablock()?, "model")?;
            Some(ShapeDraw::new(model).scaled([prop.size; 3]))
        }),
    );
    drawn
}

fn plugin_ghost(class: PluginClass, prop: Prop) -> ghost_world::world::Ghost<TgeClientSet> {
    ghost(
        TgeClass::Plugin(class),
        TgeRecord::Plugin(class, Box::new(prop)),
    )
}

/// A world of one StaticShape at `xf`, and a world of one plugin ghost there, both drawing the crate.
fn same_shape_both_ways(xf: Xf) -> (ClientWorld<TgeClientSet>, ClientWorld<TgeClientSet>, Files) {
    let (mut tge, files) = world([0.0; 3]);
    if let Some(TgeRecord::StaticShape(r)) = tge.ghosts.get_mut(&1).and_then(|g| g.record.as_mut())
    {
        r.transform = Some((xf, [F32::new(1.0); 3]));
    }
    let class = prop_class(Replicate::Interpolated);
    let mut plugin = ClientWorld::<TgeClientSet> {
        plugin_shapes: drawn_props(class),
        ..Default::default()
    };
    plugin
        .datablocks
        .insert(3, datablock("PropData", &[("model", Value::str(CRATE))]));
    let prop = Prop {
        pose: xf,
        data: DatablockRef(Some(3)),
        size: 1.0,
    };
    plugin.ghosts.insert(1, plugin_ghost(class, prop));
    (tge, plugin, files)
}

fn set_prop(w: &mut ClientWorld<TgeClientSet>, prop: Prop) {
    if let Some(g) = w.ghosts.get_mut(&1) {
        g.updates += 1;
        if let Some(TgeRecord::Plugin(_, r)) = g.record.as_mut() {
            *r = Box::new(prop);
        }
    }
}

#[test]
fn a_plugin_ghost_is_drawn_as_a_static_shape_at_the_same_pose_is() {
    let xf = pose([3.0, -2.0, 1.5], 0.7);
    let (tge, plugin, files) = same_shape_both_ways(xf);
    let (a, _) = draws_of(&tge, &files, &(), ShapeSelect::All, None);
    let (b, notes) = draws_of(&plugin, &files, &(), ShapeSelect::All, None);
    assert!(
        !a.opaque.is_empty() && a.opaque.len() == b.opaque.len(),
        "{notes:?}"
    );
    assert_eq!(a.transforms, b.transforms);
    for (x, y) in a.opaque.iter().zip(&b.opaque) {
        assert_eq!(x.vertices, y.vertices);
        assert_eq!(x.texture, y.texture);
        assert_eq!((x.blend, x.cull, x.lighting), (y.blend, y.cull, y.lighting));
    }
    assert!(b.opaque[0].label.contains(&format!("Prop 1 {CRATE}")));
    assert_eq!(b.opaque_owners, vec![1; b.opaque.len()]);
}

#[test]
fn a_plugin_ghost_needs_no_datablock_of_the_tge_kind_and_is_scaled_by_its_draw() {
    let (_, mut plugin, files) = same_shape_both_ways(pose([0.0; 3], 0.0));
    let class = prop_class(Replicate::Interpolated);
    // The class's draw function names the shape itself (with a path as a Windows file would have it).
    let mut drawn = DrawnShapes::default();
    drawn.insert(
        class,
        ShapeFn::new::<Prop>(|prop, _| {
            Some(ShapeDraw::new("demo\\data\\shapes\\crate.dts").scaled([prop.size; 3]))
        }),
    );
    plugin.plugin_shapes = drawn;
    plugin.datablocks.clear();
    let prop = Prop {
        pose: pose([5.0, 0.0, 0.0], 0.0),
        data: DatablockRef(None),
        size: 3.0,
    };
    set_prop(&mut plugin, prop);
    let (out, notes) = draws_of(&plugin, &files, &(), ShapeSelect::All, None);
    assert!(!out.opaque.is_empty(), "{notes:?}");
    let xs: Vec<f32> = out
        .opaque
        .iter()
        .flat_map(|d| d.vertices.iter().map(|v| v.position[0]))
        .collect();
    // The box is 2 across, drawn 3 times as large around x = 5.
    assert!(xs.iter().all(|x| (1.9..=8.1).contains(x)), "{xs:?}");
    assert!(xs.iter().any(|x| *x > 7.9) && xs.iter().any(|x| *x < 2.1));
}

#[test]
fn a_plugin_ghost_that_says_nothing_is_not_drawn_whatever_its_datablock_holds() {
    let (_, mut plugin, files) = same_shape_both_ways(pose([0.0; 3], 0.0));
    // A TGE-style `shapeFile` on its datablock does not draw it: only its class's function does.
    plugin.datablocks.insert(
        3,
        datablock("PropData", &[("shapeFile", Value::str(CRATE))]),
    );
    let (out, _) = draws_of(&plugin, &files, &(), ShapeSelect::All, None);
    assert!(out.opaque.is_empty());
    // Nor does a scale that is not a number.
    plugin
        .datablocks
        .insert(3, datablock("PropData", &[("model", Value::str(CRATE))]));
    let prop = Prop {
        pose: Xf::default(),
        data: DatablockRef(Some(3)),
        size: f32::NAN,
    };
    set_prop(&mut plugin, prop);
    let (out, _) = draws_of(&plugin, &files, &(), ShapeSelect::All, None);
    assert!(out.opaque.is_empty());
}

#[test]
fn a_plugin_ghost_is_a_moving_static_shape_of_the_frame() {
    let (tge, mut plugin, _) = same_shape_both_ways(pose([0.0; 3], 0.0));
    let class = prop_class(Replicate::Interpolated);
    assert!(!tge.drawn_class(TgeClass::StaticShape) && !tge.interpolated_class(TgeClass::Item));
    assert!(plugin.drawn_class(TgeClass::Plugin(class)));
    assert!(plugin.interpolated_class(TgeClass::Plugin(class)));
    assert_eq!(plugin.kind(TgeClass::Plugin(class)), ShapeKind::Static);
    assert_eq!(
        plugin.shape_classes().last(),
        Some(&TgeClass::Plugin(class))
    );
    // A live window draws it every frame; a world that does not animate leaves it with the static scene.
    assert!(is_moving(&plugin, 1) && !is_dynamic(&plugin, 1));
    plugin.animate = true;
    assert!(is_dynamic(&plugin, 1));
    // A class the client runs ahead of the server is not blended.
    let predicted = prop_class(Replicate::Predicted);
    let run = ClientWorld::<TgeClientSet> {
        plugin_shapes: drawn_props(predicted),
        ..Default::default()
    };
    assert!(run.drawn_class(TgeClass::Plugin(predicted)));
    assert!(!run.interpolated_class(TgeClass::Plugin(predicted)));
}

#[test]
fn a_plugin_ghost_is_shown_between_its_updates_and_a_world_that_does_not_animate_shows_the_record()
{
    let (_, mut plugin, files) = same_shape_both_ways(pose([0.0; 3], 0.0));
    let at = |x: f32| Prop {
        pose: pose([x, 0.0, 0.0], 0.0),
        data: DatablockRef(Some(3)),
        size: 1.0,
    };
    let drawn_x = |w: &ClientWorld<TgeClientSet>| {
        let (out, _) = draws_of(w, &files, &(), ShapeSelect::All, None);
        out.transforms[&1].column(3)[0]
    };
    // Not animating: where the record puts it, at once.
    set_prop(&mut plugin, at(4.0));
    assert_eq!(drawn_x(&plugin), 4.0);
    // Animating: first seen at 4, then an update to 8 is blended in over a tick or more.
    plugin.animate = true;
    plugin.anim_ms = 1000;
    assert_eq!(drawn_x(&plugin), 4.0);
    plugin.anim_ms = 1032;
    set_prop(&mut plugin, at(8.0));
    assert_eq!(drawn_x(&plugin), 4.0);
    plugin.anim_ms = 1048;
    assert!((drawn_x(&plugin) - 6.0).abs() < 1e-3);
    plugin.anim_ms = 1100;
    assert_eq!(drawn_x(&plugin), 8.0);
}

#[test]
fn a_plugin_ghosts_skin_is_the_texture_its_first_material_shows() {
    use torque_shape_builder::{FULL_UV, MeshData, ShapeBuilder};
    let mut b = ShapeBuilder::new();
    let node = b.node("box", None, [0.0; 3]);
    let drawn = b.detail("detail1", 1.0);
    let tex = b.material("tex", 0);
    let mut mesh = MeshData::new(Some(tex));
    mesh.add_box([-0.5; 3], [0.5; 3], |_| FULL_UV);
    b.object("box", node, drawn, mesh);
    let files = Files(
        [
            (
                "demo/box.dts".to_string(),
                shapes::write::write_dts(&b.build()),
            ),
            (
                "demo/tex.png".to_string(),
                crate::textures::tests::png(4, 4, 200),
            ),
            (
                "demo/skin.png".to_string(),
                crate::textures::tests::png(4, 4, 90),
            ),
        ]
        .into(),
    );
    let class = prop_class(Replicate::Interpolated);
    let textures_of = |skin: Option<&'static str>| {
        let mut w = ClientWorld::<TgeClientSet>::default();
        let mut drawn = DrawnShapes::default();
        drawn.insert(
            class,
            ShapeFn::new::<Prop>(move |_, _| {
                let draw = ShapeDraw::new("demo/box.dts");
                Some(match skin {
                    Some(skin) => draw.skinned(skin),
                    None => draw,
                })
            }),
        );
        w.plugin_shapes = drawn;
        w.ghosts.insert(1, plugin_ghost(class, Prop::default()));
        let mut textures = TextureSet::new(&files);
        let mut notes = Vec::new();
        let out = draws_culled(
            &w,
            &(),
            &files,
            &mut textures,
            &mut notes,
            DetailPolicy::Highest,
            ShapeSelect::All,
            None,
        );
        assert!(!out.opaque.is_empty(), "{notes:?}");
        out.opaque
            .iter()
            .filter_map(|d| d.texture)
            .map(|(i, _)| textures.path(i).unwrap_or_default().to_string())
            .collect::<Vec<_>>()
    };
    let own = textures_of(None);
    let skinned = textures_of(Some("demo/skin.png"));
    assert!(
        !own.is_empty() && own.iter().all(|p| p.ends_with("tex.png")),
        "{own:?}"
    );
    assert!(
        !skinned.is_empty() && skinned.iter().all(|p| p.ends_with("skin.png")),
        "{skinned:?}"
    );
}

/// A world of `n` StaticShapes of the solid box (ghosts 1 to `n`) in a row along y, each its own shape file when `distinct`.
fn crowd(n: u32, distinct: bool) -> (ClientWorld<TgeClientSet>, Files) {
    let (one, files) = world([0.0; 3]);
    let dts = files.0[CRATE].clone();
    let mut w = one.clone();
    let mut all = HashMap::new();
    w.ghosts.clear();
    for i in 1..=n {
        let path = if distinct {
            format!("demo/data/shapes/crate{i}.dts")
        } else {
            CRATE.to_string()
        };
        all.insert(path.clone(), dts.clone());
        let db = 1000 + i;
        w.datablocks.insert(
            db,
            datablock("StaticShapeData", &[("shapeFile", Value::str(&path))]),
        );
        let mut g = one.ghosts[&1].clone();
        if let Some(TgeRecord::StaticShape(s)) = g.record.as_mut() {
            s.shape.game.datablock = Some(db);
            if let Some((xf, _)) = s.transform.as_mut() {
                xf.position = [0.0, 3.0 * i as f32, 0.0].map(F32::new);
            }
        }
        w.ghosts.insert(i, g);
    }
    (w, Files(all))
}

fn vertices(out: &ShapeDraws) -> usize {
    out.opaque.iter().map(|d| d.vertices.len()).sum::<usize>()
        + out
            .translucent
            .iter()
            .flat_map(|(_, ds)| ds)
            .map(|d| d.vertices.len())
            .sum::<usize>()
}

#[test]
fn a_frames_moving_shapes_stay_within_the_vertex_budget_nearest_first() {
    let (w, files) = crowd(4096, false);
    let (all, _) = draws_of(&w, &files, &(), ShapeSelect::All, None);
    let each = vertices(&all) / 4096;
    assert!(each > 0);
    let budget = ShapeBudget {
        max_vertices: 100 * each,
        max_parses: None,
    };
    let (out, _) = with_budget(budget, None, || {
        draws_of(&w, &files, &(), ShapeSelect::All, None)
    });
    assert!(vertices(&out) <= budget.max_vertices);
    assert_eq!(out.over_budget, 4096 - 100);
    // The nearest ones are drawn (the camera is at the origin, the row runs away along y).
    let drawn: std::collections::BTreeSet<u32> = out.opaque_owners.iter().copied().collect();
    assert_eq!(drawn, (1..=100).collect());
}

#[test]
fn a_frame_parses_at_most_its_budget_of_new_shapes_and_the_rest_wait() {
    crate::shapes::clear_loaded();
    let (w, files) = crowd(300, true);
    let budget = ShapeBudget {
        max_vertices: usize::MAX,
        max_parses: Some(4),
    };
    let mut drawn = 0;
    for frame in 1..=3 {
        let before = crate::shapes::loaded_stats().0;
        let (out, notes) = with_budget(budget, None, || {
            draws_of(&w, &files, &(), ShapeSelect::All, None)
        });
        let loaded = crate::shapes::loaded_stats().0 - before;
        assert!(loaded <= 4, "frame {frame}: {loaded} parsed");
        assert!(notes.iter().any(|n| n.contains("put off")));
        let now: std::collections::BTreeSet<u32> = out.opaque_owners.iter().copied().collect();
        assert!(
            now.len() > drawn,
            "frame {frame}: the shapes loaded before are drawn"
        );
        drawn = now.len();
    }
    assert_eq!(drawn, 12);
    crate::shapes::clear_loaded();
}

#[test]
fn ghosts_of_one_shape_posed_the_same_share_one_kept_build() {
    let (w, files) = crowd(100, false);
    let (out, _) = crate::shapecache::with_cache(|| {
        with_nodes(true, || draws_of(&w, &files, &(), ShapeSelect::All, None))
    });
    let stamps: std::collections::BTreeSet<u64> =
        out.calls.values().flatten().map(|c| c.stamp).collect();
    assert_eq!(out.calls.len(), 100);
    assert_eq!(stamps.len(), 1, "{stamps:?}");
    assert_eq!(crate::shapecache::kept_builds(), 1);
}

#[test]
fn the_control_objects_shape_loads_whatever_the_parse_budget_has_left() {
    crate::shapes::clear_loaded();
    let (mut w, files) = crowd(300, true);
    // The control object comes last in the ghosts' order, after 299 shapes nothing has parsed yet.
    w.control.object = Some(300);
    let budget = ShapeBudget {
        max_vertices: usize::MAX,
        max_parses: Some(4),
    };
    let before = crate::shapes::loaded_stats().0;
    let (out, _) = with_budget(budget, None, || {
        draws_of(&w, &files, &(), ShapeSelect::All, None)
    });
    assert!(
        out.opaque_owners.contains(&300),
        "the control object is drawn"
    );
    assert_eq!(
        crate::shapes::loaded_stats().0 - before,
        5,
        "four parses and the control object's"
    );
    assert_eq!(out.put_off, 299 - 4);
    crate::shapes::clear_loaded();
}

#[test]
fn only_the_control_object_the_vehicle_it_drives_and_its_mounts_load_past_the_parse_budget() {
    crate::shapes::clear_loaded();
    let (mut w, files) = crowd(300, true);
    w.control.object = Some(300);
    // A vehicle the client runs on its own is not exempt; the one it drives is.
    w.vehicle_render
        .insert(299, w.ghosts[&1].pose().expect("a pose"));
    let budget = ShapeBudget {
        max_vertices: usize::MAX,
        max_parses: Some(0),
    };
    let before = crate::shapes::loaded_stats().0;
    let (out, _) = with_budget(budget, Some(298), || {
        draws_of(&w, &files, &(), ShapeSelect::All, None)
    });
    let drawn: std::collections::BTreeSet<u32> = out.opaque_owners.iter().copied().collect();
    assert_eq!(drawn, [298, 300].into());
    assert_eq!(crate::shapes::loaded_stats().0 - before, 2);
    crate::shapes::clear_loaded();
}

/// One frame of `w`'s objects as a window's control sync makes them: built in the shape cache, made objects of with the kept
/// objects, given their motion.
fn kept_frame(
    w: &ClientWorld<TgeClientSet>,
    files: &Files,
    drawn: &mut Vec<crate::hd::DynamicObject>,
    kept: &mut HashMap<u32, crate::hd::objects::KeptObject>,
) {
    let (shaped, _) =
        crate::shapecache::with_cache(|| draws_of(w, files, &(), ShapeSelect::All, None));
    let mut out = crate::hd::objects::objects_of(w, &shaped, &[], drawn, Some(kept));
    crate::hd::objects::carry_motion(&mut out, drawn);
    *drawn = out;
}

#[test]
fn a_ghost_posed_as_it_was_takes_its_last_objects_draws_and_converts_nothing() {
    let (mut w, files) = world([0.0; 3]);
    let (mut drawn, mut kept) = (Vec::new(), HashMap::new());
    kept_frame(&w, &files, &mut drawn, &mut kept);
    let stamp = drawn[0].stamp.expect("a kept object has a stamp");
    let vertices = drawn[0].draws[0].vertices.as_ptr();
    // The same pose: the same object, its draws moved over (not converted again), still where they were.
    kept_frame(&w, &files, &mut drawn, &mut kept);
    assert_eq!(drawn[0].stamp, Some(stamp));
    assert_eq!(drawn[0].draws[0].vertices.as_ptr(), vertices);
    let here: Vec<[f32; 3]> = drawn[0]
        .draws
        .iter()
        .flat_map(|d| d.vertices.iter().map(|v| v.position))
        .collect();
    assert_eq!(drawn[0].previous.as_deref(), Some(here.as_slice()));
    // Moved: built again, a new stamp, its vertices' last places the last frame's.
    if let Some(TgeRecord::StaticShape(s)) = w.ghosts.get_mut(&1).and_then(|g| g.record.as_mut())
        && let Some((xf, _)) = s.transform.as_mut()
    {
        xf.position = [1.0, 0.0, 0.0].map(F32::new);
    }
    kept_frame(&w, &files, &mut drawn, &mut kept);
    assert_ne!(drawn[0].stamp, Some(stamp));
    let last = drawn[0].previous.as_ref().expect("the last frame's places");
    assert!(last.iter().zip(&here).all(|(a, b)| a == b));
}

/// Hooks that keep a live pose for every ghost and count the poses advanced and posed, and advanced unseen.
#[derive(Default)]
struct Clock {
    posed: std::sync::atomic::AtomicUsize,
    unseen: std::sync::atomic::AtomicUsize,
}

impl<W: ShapeWorld + ?Sized> Hooks<W> for Clock {
    type Live = ();

    fn take_live(&self, _: u32) -> Option<()> {
        Some(())
    }

    fn advance_live(
        &self,
        _: &W,
        _: LiveAsk<'_, W>,
        kept: Option<()>,
        _: &mut Vec<String>,
    ) -> (Option<()>, Option<ShapeInstance>) {
        self.posed
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        (kept, None)
    }

    fn advance_unseen(
        &self,
        _: &W,
        _: LiveAsk<'_, W>,
        kept: Option<()>,
        _: &mut Vec<String>,
    ) -> Option<()> {
        self.unseen
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        kept
    }
}

#[test]
fn a_player_the_cull_leaves_out_keeps_time_but_is_not_posed_or_built() {
    let (mut w, files) = world([0.0; 3]);
    w.datablocks.insert(
        4,
        datablock("PlayerData", &[("shapeFile", Value::str(CRATE))]),
    );
    let record = TgeRecord::Player(ghosts::player::Player {
        shape: TgeShapeBase {
            game: ghosts::parts::GameBase {
                scale: None,
                datablock: Some(4),
            },
            body: None,
        },
        motion: Some(ghosts::player::PlayerMove::default()),
        ..Default::default()
    });
    let mut g = ghost(TgeClass::Player, record);
    g.position = Some([0.0, 5.0, 0.0]);
    w.ghosts.insert(2, g);
    w.animate = true;
    let count = |h: &Clock| {
        use std::sync::atomic::Ordering::Relaxed;
        (h.posed.load(Relaxed), h.unseen.load(Relaxed))
    };
    let hooks = Clock::default();
    let (out, _) = draws_of(&w, &files, &hooks, ShapeSelect::All, Some(&Nothing));
    assert_eq!(count(&hooks), (0, 1), "advanced, not posed");
    assert_eq!(out.culled, 2);
    assert!(out.opaque.is_empty() && out.translucent.is_empty());
    let (out, _) = draws_of(&w, &files, &hooks, ShapeSelect::All, None);
    assert_eq!(count(&hooks), (1, 1));
    assert!(out.opaque_owners.contains(&2));
}

/// A host that draws ghost 1 with its still geometry.
struct KeepsOneStill;

impl Hooks<ClientWorld<TgeClientSet>> for KeepsOneStill {
    type Live = ();

    fn still(&self, index: u32) -> bool {
        index == 1
    }
}

#[test]
fn a_resting_plugin_ghost_is_left_out_only_where_its_host_draws_it_still() {
    let (_, mut plugin, files) = same_shape_both_ways(pose([2.0, 0.0, 1.0], 0.4));
    let class = prop_class(Replicate::Interpolated);
    // Resting while its size is above 0; a size of 2 is drawn twice as long along x alone.
    let mut drawn = DrawnShapes::default();
    drawn.insert(
        class,
        ShapeFn::new::<Prop>(|prop, _| {
            let draw = ShapeDraw::new(CRATE).resting(prop.size > 0.0);
            Some(if prop.size == 2.0 {
                draw.scaled([2.0, 1.0, 1.0])
            } else {
                draw
            })
        }),
    );
    plugin.plugin_shapes = drawn;
    assert!(resting(&plugin, 1));
    // A host that keeps it still: left out of the frame's shapes; a host that does not (it could not): drawn as every frame.
    let (kept, _) = draws_of(&plugin, &files, &KeepsOneStill, ShapeSelect::Moving, None);
    let (drawn_here, _) = draws_of(&plugin, &files, &(), ShapeSelect::Moving, None);
    assert!(kept.opaque.is_empty() && !drawn_here.opaque.is_empty());
    // A prediction drawing it somewhere else: it does not rest.
    let at = plugin.ghosts[&1].pose().expect("a pose");
    plugin.ghost_render.insert(1, at);
    assert!(!resting(&plugin, 1));
    plugin.ghost_render.clear();
    // Not resting, or a scale a still copy cannot have.
    for size in [-1.0, 2.0] {
        let prop = Prop {
            pose: pose([2.0, 0.0, 1.0], 0.4),
            data: DatablockRef(Some(3)),
            size,
        };
        set_prop(&mut plugin, prop);
        assert!(!resting(&plugin, 1), "size {size}");
    }
}
