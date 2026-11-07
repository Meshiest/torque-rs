//! Debug gizmos drawn offscreen over a small scene: each kind of shape, the depth test on and off, and a label, as pictures and as
//! counts of the pixels they changed from the same scene drawn with none. `TORQUE_GIZMO_PICTURES=<folder>` also writes the
//! pictures there as PNGs.
#![cfg(feature = "gizmos")]

use std::path::PathBuf;
use std::rc::Rc;

use glam::{Quat, Vec3};
use torque_app::gizmos::{GizmoPainter, GizmoSettings, Gizmos, Style, project};
use torque_app::shots::write_png;
use torque_app::view::{Daylight, WorldView};
use torque_render_core::{Frame, Gpu};
use torque_render_scene::hd::HdCamera;
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::source::Eye;

const SIZE: (u32, u32) = (960, 540);
/// Frames of a sixtieth of a second each picture settles for (the anti-aliasing's history).
const FRAMES: u32 = 10;

fn gpu() -> Option<Gpu> {
    match torque_app::window::offscreen_gpu() {
        Ok(gpu) => Some(gpu),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

fn folder() -> Option<PathBuf> {
    std::env::var_os("TORQUE_GIZMO_PICTURES").map(PathBuf::from)
}

/// A view of a plain floor under the default day, with a wall standing across the middle when asked.
fn view(gpu: &Gpu, wall: bool) -> WorldView {
    let mut view = WorldView::new(gpu);
    view.set_plain_floor(80.0, [128, 150, 110]);
    view.set_daylight(&Daylight::default());
    if wall {
        let mut slab = box_object(1, [0.0, 0.0, 2.0], [3.0, 0.3, 2.0]);
        slab.draws[0]
            .vertices
            .iter_mut()
            .for_each(|v| v.color = [196, 188, 172, 255]);
        view.set_objects(vec![slab]);
    }
    view
}

fn eye() -> Eye {
    look_at([0.0, -16.0, 4.5], [0.0, 0.0, 2.0])
}

/// Where the camera of the second depth picture stands: to one side and above, so a box behind the wall is hidden by it.
fn high_eye() -> Eye {
    look_at([2.5, -9.0, 5.5], [0.0, 0.0, 1.8])
}

/// The scene drawn for `FRAMES` frames from `eye`, the shapes in `gizmos` (long lived) over it.
fn shot_from(view: &mut WorldView, gpu: &Gpu, eye: Eye) -> Frame {
    view.set_gizmo_frame_seconds(Some(1.0 / 60.0));
    view.reset_history();
    let mut frame = None;
    for _ in 0..FRAMES {
        frame = Some(
            view.render_world(gpu, SIZE, eye)
                .expect("the world is drawn"),
        );
    }
    frame.unwrap()
}

fn shot(view: &mut WorldView, gpu: &Gpu) -> Frame {
    shot_from(view, gpu, eye())
}

fn save(frame: &Frame, name: &str) {
    if let Some(dir) = folder() {
        std::fs::create_dir_all(&dir).unwrap();
        write_png(frame, &dir.join(format!("{name}.png"))).unwrap();
    }
}

/// The pixels of `rect` (left, top, width, height) that differ clearly between two pictures.
fn changed(a: &Frame, b: &Frame, rect: [i32; 4]) -> usize {
    let [x0, y0, w, h] = rect;
    let mut n = 0;
    for y in y0.max(0)..(y0 + h).min(a.height as i32) {
        for x in x0.max(0)..(x0 + w).min(a.width as i32) {
            let i = 4 * (y as u32 * a.width + x as u32) as usize;
            let d: i32 = (0..3)
                .map(|c| (i32::from(a.rgba[i + c]) - i32::from(b.rgba[i + c])).abs())
                .max()
                .unwrap();
            if d > 40 {
                n += 1;
            }
        }
    }
    n
}

/// The pixel rectangle of `half` pixels either side of the world point `p`.
fn around(p: [f32; 3], half: i32) -> [i32; 4] {
    around_from(eye(), p, half)
}

fn around_from(eye: Eye, p: [f32; 3], half: i32) -> [i32; 4] {
    let camera = HdCamera::new(eye, 90.0_f32, 1000.0, SIZE.0, SIZE.1);
    let (x, y) = project(&camera, p, [0, 0, SIZE.0, SIZE.1]).expect("the point is in view");
    [x as i32 - half, y as i32 - half, 2 * half, 2 * half]
}

const FOREVER: f32 = 1_000.0;

fn style(color: [f32; 4]) -> Style {
    Style::new(color).seconds(FOREVER)
}

fn fonts_found() -> bool {
    torque_app::draw_cache()
        .text_size(GizmoSettings::default().font_face, 14, b"x")
        .is_some()
}

#[test]
fn every_kind_of_shape_is_drawn_and_a_label_with_it() {
    let Some(gpu) = gpu() else { return };
    let mut plain = view(&gpu, false);
    let before = shot(&mut plain, &gpu);

    let mut view = view(&gpu, false);
    let g = Rc::new(Gizmos::new(GizmoSettings::default()));
    view.attach_gizmos(Rc::clone(&g));
    let yellow = style([1.0, 0.9, 0.1, 1.0]);
    let cyan = style([0.1, 0.9, 1.0, 1.0]);
    let magenta = style([1.0, 0.2, 0.9, 1.0]);
    let red = style([1.0, 0.15, 0.1, 1.0]);
    let white = style([1.0; 4]);
    let blue = style([0.3, 0.5, 1.0, 1.0]);
    g.line([-13.0, 0.0, 0.5], [-9.5, 0.0, 3.5], yellow);
    g.label([-11.2, 0.0, 3.6], "line", white);
    g.aabb([-8.0, -1.0, 0.2], [-5.0, 1.0, 2.8], cyan);
    g.label([-6.5, 0.0, 3.0], "box", white);
    let turn = Quat::from_axis_angle(Vec3::Z, 0.6) * Quat::from_axis_angle(Vec3::X, 0.35);
    g.obb([-2.2, 0.0, 1.6], [1.4, 0.8, 1.0], turn, magenta);
    g.label([-2.2, 0.0, 3.2], "oriented box", white);
    g.sphere([1.6, 0.0, 1.6], 1.5, red);
    g.label([1.6, 0.0, 3.3], "sphere", white);
    g.arrow([4.5, 0.0, 0.3], [7.2, 0.0, 3.0], blue);
    g.label([5.9, 0.0, 3.2], "arrow", white);
    g.cross([10.0, 0.0, 1.6], 1.4, yellow);
    g.label([10.0, 0.0, 3.2], "cross", white);
    assert_eq!(g.label_count(), 6);
    let after = shot(&mut view, &gpu);
    save(&after, "kinds");

    for (name, p) in [
        ("line", [-11.2, 0.0, 2.0]),
        ("box", [-6.5, 0.0, 1.5]),
        ("oriented box", [-2.2, 0.0, 1.6]),
        ("sphere", [1.6, 0.0, 1.6]),
        ("arrow", [5.9, 0.0, 1.6]),
        ("cross", [10.0, 0.0, 1.6]),
    ] {
        let n = changed(&before, &after, around(p, 55));
        assert!(n > 150, "{name}: {n} pixels changed");
    }
    if fonts_found() {
        // Above the sphere's top (z 3.1) the only change is its label.
        let n = changed(&before, &after, around([1.6, 0.0, 3.9], 20));
        assert!(n > 20, "the label: {n} pixels changed");
    }
    // Nothing is left asked for after the long lived shapes were drawn for ten frames: they stay.
    assert!(!g.is_empty());
}

#[test]
fn a_shape_behind_the_wall_is_hidden_with_the_depth_test_and_shows_without_it() {
    let Some(gpu) = gpu() else { return };
    let mut plain = view(&gpu, true);
    let before = shot(&mut plain, &gpu);

    let green = style([0.2, 1.0, 0.2, 1.0]);
    let red = style([1.0, 0.2, 0.2, 1.0]);
    let draw = |g: &Gizmos| {
        // Behind the wall (y 4): the tested lines vanish where the wall is.
        g.line([-8.0, 4.0, 1.0], [8.0, 4.0, 1.0], green);
        g.line([-8.0, 4.0, 3.0], [8.0, 4.0, 3.0], green.depth(true));
        // The same, not tested.
        g.line([-8.0, 4.0, 1.9], [8.0, 4.0, 1.9], red.depth(false));
        // A sphere through the wall: the front of it shows, the back half is behind.
        g.sphere([0.0, 0.0, 2.0], 1.6, style([0.1, 0.9, 1.0, 1.0]));
        g.label([0.0, 0.0, 4.4], "depth test", style([1.0; 4]));
    };
    let mut view = view(&gpu, true);
    let g = Rc::new(Gizmos::new(GizmoSettings::default()));
    view.attach_gizmos(Rc::clone(&g));
    draw(&g);
    let after = shot(&mut view, &gpu);
    save(&after, "depth");

    // On the wall's face, where the tested lines pass behind it: unchanged.
    // (Clear of the sphere, whose rings are a pixel's width across the wall's face.)
    let behind_wall = [-2.5, 4.0, 1.0];
    assert_eq!(changed(&before, &after, around(behind_wall, 5)), 0);
    let behind_wall = [2.5, 4.0, 3.0];
    assert_eq!(changed(&before, &after, around(behind_wall, 5)), 0);
    // Past the wall's edge the same lines show.
    let past = [-6.0, 4.0, 1.0];
    assert!(changed(&before, &after, around(past, 10)) > 30);
    // The line drawn without the test shows across the wall.
    let through = [-2.5, 4.0, 1.9];
    assert!(changed(&before, &after, around(through, 6)) > 20);
}

#[test]
fn two_boxes_behind_a_wall_one_tested_and_one_not() {
    let Some(gpu) = gpu() else { return };
    let mut plain = view(&gpu, true);
    let before = shot_from(&mut plain, &gpu, high_eye());

    let mut view = view(&gpu, true);
    let g = Rc::new(Gizmos::new(GizmoSettings::default()));
    view.attach_gizmos(Rc::clone(&g));
    let green = style([0.2, 1.0, 0.2, 1.0]);
    let red = style([1.0, 0.25, 0.2, 1.0]);
    g.aabb([-2.6, 2.0, 0.3], [-0.6, 4.0, 3.0], green);
    g.aabb([0.6, 2.0, 0.3], [2.6, 4.0, 3.0], red.depth(false));
    g.label([-1.6, 0.0, 4.2], "depth test on", style([1.0; 4]));
    g.label([1.6, 0.0, 4.2], "depth test off", style([1.0; 4]));
    let after = shot_from(&mut view, &gpu, high_eye());
    save(&after, "depth-boxes");

    let hidden = around_from(high_eye(), [-1.6, 3.0, 1.6], 50);
    assert_eq!(changed(&before, &after, hidden), 0, "the tested box");
    let shown = around_from(high_eye(), [1.6, 3.0, 1.6], 50);
    assert!(
        changed(&before, &after, shown) > 100,
        "the box drawn over the wall"
    );
}

#[test]
fn a_set_with_nothing_in_it_does_no_gpu_work() {
    let Some(gpu) = gpu() else { return };
    let target = gpu.offscreen(64, 64).unwrap();
    let camera = HdCamera::new(eye(), 90.0_f32, 100.0, 64, 64);
    let g = Rc::new(Gizmos::new(GizmoSettings::default()));
    let mut painter = GizmoPainter::new(Rc::clone(&g));
    painter.set_frame_seconds(Some(0.0));
    let (before, _) = torque_render_hd::passes_by_kind();
    for _ in 0..3 {
        painter.paint(&gpu, &target.color, [0, 0, 64, 64], &camera, None);
    }
    assert!(painter.is_idle(), "no pipeline, buffer or font was made");
    assert_eq!(torque_render_hd::passes_by_kind().0, before);

    g.line([0.0, 0.0, 0.0], [1.0, 0.0, 1.0], Style::default());
    painter.paint(&gpu, &target.color, [0, 0, 64, 64], &camera, None);
    assert!(!painter.is_idle());
    // One batched pass for the lines, and the one-frame line is gone.
    let after = torque_render_hd::passes_by_kind().0;
    assert_eq!(after[6], before[6] + 1);
    assert!(g.is_empty());
}

#[test]
fn a_view_with_the_set_but_nothing_in_it_draws_what_one_without_does() {
    let Some(gpu) = gpu() else { return };
    let mut plain = view(&gpu, true);
    let a = shot(&mut plain, &gpu);
    let mut with = view(&gpu, true);
    with.attach_gizmos(Rc::new(Gizmos::new(GizmoSettings::default())));
    let b = shot(&mut with, &gpu);
    assert_eq!(a.rgba, b.rgba);
}
