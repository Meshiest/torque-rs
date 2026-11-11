use glam::{Quat, Vec3};
use torque_engine::{ClientStage, ClientTick, Engine, EngineBuilder};
use torque_ts_vm::testing::{eval, log_lines};
use torque_ts_vm::vm::Vm;

use super::*;
use crate::client::{ClientBuilder, ClientNet};
use crate::plugins::DefaultPlugins;

const RED: Style = Style::new([1.0, 0.0, 0.0, 1.0]);

fn gizmos() -> Gizmos {
    Gizmos::new(GizmoSettings::default())
}

fn engine(settings: GizmoSettings) -> Engine {
    EngineBuilder::new()
        .add_plugins(GizmosPlugin::new(settings))
        .build()
        .expect("the engine builds")
}

fn client_vm(engine: &Engine) -> Vm {
    let mut vm = Vm::new();
    for &stage in ClientStage::ALL {
        engine.replay(stage, &mut vm).expect("the recipes run");
    }
    vm
}

fn tick(engine: &Engine, vm: &mut Vm) {
    for hook in engine.list::<ClientTick>() {
        hook.run(vm);
    }
}

fn segments(g: &Gizmos) -> Vec<torque_render_hd::gizmos::GizmoSegment> {
    g.with_drawn(|s, _| s.to_vec())
}

#[test]
fn a_shape_asked_for_one_frame_is_drawn_once() {
    let g = gizmos();
    g.line([0.0; 3], [1.0, 0.0, 0.0], RED);
    g.label([0.0; 3], "here", RED);
    assert_eq!((g.line_count(), g.label_count()), (1, 1));
    // A frame that took no time still ends the shapes of one frame.
    g.end_frame(0.0);
    assert!(g.is_empty());
}

#[test]
fn a_timed_shape_stays_for_its_seconds_of_drawn_frames() {
    let g = gizmos();
    g.line([0.0; 3], [1.0, 0.0, 0.0], RED.seconds(1.0));
    g.label([0.0; 3], "half a second", RED.seconds(0.5));
    g.line([0.0; 3], [0.0, 1.0, 0.0], RED);
    g.end_frame(0.25);
    assert_eq!((g.line_count(), g.label_count()), (1, 1));
    g.end_frame(0.25);
    // The label's half second is spent; the line's second is half spent.
    assert_eq!((g.line_count(), g.label_count()), (1, 0));
    g.end_frame(0.5);
    assert!(g.is_empty());
}

#[test]
fn clearing_takes_everything_and_a_time_that_is_not_a_number_is_one_frame() {
    let g = gizmos();
    g.line([0.0; 3], [1.0; 3], RED.seconds(5.0));
    g.label([0.0; 3], "x", RED.seconds(5.0));
    g.clear();
    assert!(g.is_empty());
    g.line([0.0; 3], [1.0; 3], RED.seconds(f32::NAN));
    g.line([0.0; 3], [1.0; 3], RED.seconds(-3.0));
    g.end_frame(f32::NAN);
    assert!(g.is_empty());
}

#[test]
fn the_style_sets_the_color_and_the_depth_test() {
    let g = gizmos();
    g.line([0.0; 3], [1.0; 3], Style::new([1.0, 0.5, 0.0, 2.0]));
    g.line([0.0; 3], [1.0; 3], RED.depth(false));
    g.line([0.0; 3], [1.0; 3], Style::new([f32::NAN, -1.0, 0.25, 0.0]));
    let s = segments(&g);
    assert_eq!(s[0].color, [255, 128, 0, 255]);
    assert!(s[0].depth_test);
    assert!(!s[1].depth_test);
    assert_eq!(s[2].color, [0, 0, 64, 0]);
}

#[test]
fn each_shape_makes_its_segments() {
    let g = gizmos();
    g.aabb([0.0; 3], [1.0; 3], RED);
    assert_eq!(g.line_count(), 12);
    g.clear();
    g.obb([0.0; 3], [1.0; 3], Quat::IDENTITY, RED);
    assert_eq!(g.line_count(), 12);
    g.clear();
    g.sphere([0.0; 3], 2.0, RED);
    assert_eq!(g.line_count(), 3 * shapes::CIRCLE_SEGMENTS);
    g.clear();
    g.arrow([0.0; 3], [0.0, 0.0, 4.0], RED);
    assert_eq!(g.line_count(), 5);
    g.clear();
    g.cross([0.0; 3], 1.0, RED);
    assert_eq!(g.line_count(), 3);
    // An arrow from a point to itself is a single line, not a head of NaNs.
    g.clear();
    g.arrow([1.0; 3], [1.0; 3], RED);
    assert_eq!(g.line_count(), 1);
    assert!(
        segments(&g)
            .iter()
            .all(|s| s.a.iter().chain(&s.b).all(|c| c.is_finite()))
    );
}

#[test]
fn a_box_lies_on_its_corners_and_a_turned_box_on_the_turned_ones() {
    let g = gizmos();
    // The corners given in either order.
    g.aabb([3.0, -1.0, 0.0], [1.0, 1.0, 2.0], RED);
    let lo = Vec3::new(1.0, -1.0, 0.0);
    let hi = Vec3::new(3.0, 1.0, 2.0);
    for seg in segments(&g) {
        for p in [Vec3::from(seg.a), Vec3::from(seg.b)] {
            assert!(p.cmpge(lo - 1e-5).all() && p.cmple(hi + 1e-5).all(), "{p}");
        }
    }
    // A box 2 long in x and 1 wide in y, turned a quarter about z: 2 long in y.
    let g = gizmos();
    g.obb(
        [0.0; 3],
        [1.0, 0.5, 0.5],
        Quat::from_axis_angle(Vec3::Z, std::f32::consts::FRAC_PI_2),
        RED,
    );
    let ys: Vec<f32> = segments(&g).iter().map(|s| s.a[1]).collect();
    assert!(ys.iter().any(|y| (y.abs() - 1.0).abs() < 1e-5));
    assert!(ys.iter().all(|y| y.abs() <= 1.0 + 1e-5));
}

#[test]
fn a_shape_with_a_coordinate_that_is_not_a_number_is_not_kept() {
    let g = gizmos();
    g.line([f32::NAN, 0.0, 0.0], [1.0; 3], RED);
    g.sphere([0.0; 3], f32::INFINITY, RED);
    g.label([0.0, f32::INFINITY, 0.0], "x", RED);
    assert!(g.is_empty());
}

#[test]
fn a_shape_past_the_cap_is_dropped_whole_and_warned_of_once_per_run_of_frames() {
    let g = Gizmos::new(GizmoSettings {
        max_lines: 100,
        max_labels: 2,
        ..GizmoSettings::default()
    });
    // 96 fit; the next sphere would make 192: dropped whole, not cut.
    g.sphere([0.0; 3], 1.0, RED);
    g.sphere([0.0; 3], 1.0, RED);
    g.sphere([0.0; 3], 1.0, RED);
    assert_eq!(g.line_count(), 96);
    // A line still fits in what is left.
    g.line([0.0; 3], [1.0; 3], RED);
    assert_eq!(g.line_count(), 97);
    for _ in 0..3 {
        g.label([0.0; 3], "x", RED.seconds(1.0));
    }
    assert_eq!(g.label_count(), 2);
    let warnings = g.take_warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].starts_with("gizmos: more than 100 lines"));
    // The next frame overflows too: the same run, no new warning.
    g.end_frame(0.0);
    g.sphere([0.0; 3], 1.0, RED);
    g.sphere([0.0; 3], 1.0, RED);
    assert!(g.take_warnings().is_empty());
    // A frame with no drops ends the run; the next overflow warns again.
    g.end_frame(0.0);
    g.end_frame(0.0);
    g.sphere([0.0; 3], 1.0, RED);
    g.sphere([0.0; 3], 1.0, RED);
    assert_eq!(g.take_warnings().len(), 1);
}

#[test]
fn a_long_text_is_cut_on_a_character() {
    let g = gizmos();
    g.label([0.0; 3], &"\u{e9}".repeat(300), RED);
    let text = g.with_drawn(|_, labels| labels[0].text.clone());
    assert_eq!(text.len(), MAX_LABEL_BYTES);
    assert!(text.chars().all(|c| c == '\u{e9}'));
}

#[test]
fn the_plugin_gives_a_client_its_set_and_its_natives_and_a_default_client_has_neither() {
    let engine = engine(GizmoSettings::default());
    let vm = client_vm(&engine);
    assert!(Gizmos::of(&vm).is_some());
    let listing = vm.native_listing();
    for spec in &NATIVE_SPECS {
        assert!(listing.contains(spec.name), "{}", spec.name);
    }
    let plain = EngineBuilder::new()
        .add_plugins(DefaultPlugins)
        .build()
        .expect("the engine builds");
    assert!(!plain.is_plugin_added::<GizmosPlugin>());
    let builder = ClientBuilder::from_engine(&plain, None, ClientNet::None).expect("the client");
    assert!(Gizmos::of(&builder.vm).is_none());
    assert!(!builder.vm.native_listing().contains("drawDebug"));
    assert!(!builder.vm.native_listing().contains("clearDebug"));
}

#[test]
fn a_client_over_the_default_plugins_and_this_one_has_the_set() {
    let engine = EngineBuilder::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(GizmosPlugin::default())
        .build()
        .expect("the engine builds");
    let builder = ClientBuilder::from_engine(&engine, None, ClientNet::None).expect("the client");
    let client = builder.build::<crate::GameServer>();
    let gizmos = client.gizmos().expect("the client has gizmos");
    gizmos.line([0.0; 3], [1.0; 3], RED);
    assert_eq!(client.gizmos().unwrap().line_count(), 1);
}

#[test]
fn the_natives_read_their_arguments_as_tge_does() {
    let engine = engine(GizmoSettings::default());
    let mut vm = client_vm(&engine);
    let g = Gizmos::of(&vm).unwrap();
    eval(
        &mut vm,
        r#"drawDebugLine("0 0 0", "1 2 3", "1 0.5 0", 2.5, 0);"#,
    );
    let s = segments(&g);
    assert_eq!(s.len(), 1);
    assert_eq!((s[0].a, s[0].b), ([0.0; 3], [1.0, 2.0, 3.0]));
    // The alpha left out is 1.
    assert_eq!(s[0].color, [255, 128, 0, 255]);
    assert!(!s[0].depth_test);
    // 2.5 seconds: alive after 2, gone after 3.
    g.end_frame(2.0);
    assert_eq!(g.line_count(), 1);
    g.end_frame(1.0);
    assert!(g.is_empty());

    // A vector short of numbers or not a number at all reads as zeros, a color may carry an alpha.
    eval(&mut vm, r#"drawDebugLine("5", "x y z", "0 0 1 0.5");"#);
    let s = segments(&g);
    assert_eq!((s[0].a, s[0].b), ([5.0, 0.0, 0.0], [0.0; 3]));
    assert_eq!(s[0].color, [0, 0, 255, 128]);
    // Left out: one frame, depth tested.
    assert!(s[0].depth_test);
    g.end_frame(0.01);
    assert!(g.is_empty());
    // An empty color is white.
    eval(&mut vm, r#"drawDebugLine("0 0 0", "1 0 0", "");"#);
    assert_eq!(segments(&g)[0].color, [255; 4]);
}

#[test]
fn every_native_draws_its_shape() {
    let engine = engine(GizmoSettings::default());
    let mut vm = client_vm(&engine);
    let g = Gizmos::of(&vm).unwrap();
    for (call, lines, labels) in [
        (r#"drawDebugBox("0 0 0", "1 1 1", "1 1 1");"#, 12, 0),
        (
            r#"drawDebugOrientedBox("0 0 0", "1 1 1", "0 0 1 0.5", "1 1 1", 1);"#,
            12,
            0,
        ),
        (r#"drawDebugSphere("0 0 0", 2, "1 1 1", 1, 0);"#, 96, 0),
        (r#"drawDebugArrow("0 0 0", "0 0 4", "1 1 1");"#, 5, 0),
        (r#"drawDebugCross("0 0 0", 1, "1 1 1");"#, 3, 0),
        (r#"drawDebugText("0 0 1", "hello world", "1 1 0");"#, 0, 1),
    ] {
        g.clear();
        eval(&mut vm, call);
        assert_eq!((g.line_count(), g.label_count()), (lines, labels), "{call}");
    }
    eval(&mut vm, "clearDebug();");
    assert!(g.is_empty());
    eval(&mut vm, r#"drawDebugText("1 2 3", "tagged", "1 0 0");"#);
    let label = g.with_drawn(|_, l| l[0].clone());
    assert_eq!(label.text, "tagged");
    assert_eq!(label.position, [1.0, 2.0, 3.0]);
    assert_eq!(label.color, [255, 0, 0, 255]);
}

#[test]
fn a_turned_box_from_a_script_is_turned_by_the_angle_axis_in_radians() {
    let engine = engine(GizmoSettings::default());
    let mut vm = client_vm(&engine);
    let g = Gizmos::of(&vm).unwrap();
    // 2 long in x, a quarter turn about z (the axis need not be a unit vector): 2 long in y.
    eval(
        &mut vm,
        r#"drawDebugOrientedBox("0 0 0", "1 0.5 0.5", "0 0 2 1.5707964", "1 1 1");"#,
    );
    let ys: Vec<f32> = segments(&g).iter().map(|s| s.a[1]).collect();
    assert!(ys.iter().any(|y| (y.abs() - 1.0).abs() < 1e-4));
    // An axis of no length is no turn.
    g.clear();
    eval(
        &mut vm,
        r#"drawDebugOrientedBox("0 0 0", "1 0.5 0.5", "0 0 0 1", "1 1 1");"#,
    );
    let ys: Vec<f32> = segments(&g).iter().map(|s| s.a[1]).collect();
    assert!(ys.iter().all(|y| y.abs() <= 0.5 + 1e-5));
}

#[test]
fn a_call_with_too_few_or_too_many_arguments_draws_nothing() {
    let engine = engine(GizmoSettings::default());
    let mut vm = client_vm(&engine);
    let g = Gizmos::of(&vm).unwrap();
    eval(&mut vm, r#"drawDebugLine("0 0 0", "1 1 1");"#);
    eval(
        &mut vm,
        r#"drawDebugLine("0 0 0", "1 1 1", "1 1 1", 1, 1, 7);"#,
    );
    eval(&mut vm, "clearDebug(1);");
    assert!(g.is_empty());
    let lines = log_lines(&mut vm);
    assert!(
        lines.iter().any(|l| l.contains("drawDebugLine(")),
        "{lines:?}"
    );
}

#[test]
fn the_console_is_told_when_a_script_overflows_the_caps() {
    let engine = engine(GizmoSettings {
        max_lines: 10,
        max_labels: 1,
        ..GizmoSettings::default()
    });
    let mut vm = client_vm(&engine);
    let g = Gizmos::of(&vm).unwrap();
    eval(&mut vm, r#"drawDebugBox("0 0 0", "1 1 1", "1 1 1");"#);
    // Twelve lines are past ten: the box was dropped and the console told at once.
    assert!(g.is_empty());
    let told = log_lines(&mut vm);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("more than 10 lines at once"));
}

#[test]
fn rust_overflows_reach_the_console_with_the_next_tick() {
    let engine = engine(GizmoSettings {
        max_lines: 10,
        max_labels: 1,
        ..GizmoSettings::default()
    });
    let mut vm = client_vm(&engine);
    let g = Gizmos::of(&vm).unwrap();
    g.label([0.0; 3], "a", RED);
    g.label([0.0; 3], "b", RED);
    assert!(log_lines(&mut vm).is_empty());
    tick(&engine, &mut vm);
    let told = log_lines(&mut vm);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("more than 1 labels at once"));
    // Told once.
    tick(&engine, &mut vm);
    assert!(log_lines(&mut vm).is_empty());
}

#[test]
fn a_point_projects_into_the_world_rectangle_and_one_behind_the_eye_does_not() {
    use torque_render_scene::hd::HdCamera;
    use torque_render_scene::source::Eye;
    let eye = Eye {
        position: [0.0, -10.0, 0.0],
        pitch: 0.0,
        yaw: 0.0,
        fov: None,
    };
    let camera = HdCamera::new(eye, 90.0_f32, 100.0, 200, 100);
    let rect = [20, 10, 200, 100];
    // Straight ahead: the middle of a rectangle that starts at (20, 10).
    let (x, y) = paint::project(&camera, [0.0, 0.0, 0.0], rect).unwrap();
    assert!(
        (x - 120.0).abs() < 0.01 && (y - 60.0).abs() < 0.01,
        "{x} {y}"
    );
    // Up is toward the top; right is toward the right.
    let (x2, y2) = paint::project(&camera, [2.0, 0.0, 2.0], rect).unwrap();
    assert!(x2 > x && y2 < y);
    assert!(paint::project(&camera, [0.0, -20.0, 0.0], rect).is_none());
    assert!(paint::project(&camera, [500.0, 0.0, 0.0], rect).is_none());
}
