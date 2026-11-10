//! The strips' view of the renderer's camera: its right, up and forward, and the size of a pixel, read from its matrices.

use glam::Vec3;
use torque_plugin_rope::strip::StripView;
use torque_render_scene::hd::HdCamera;
use torque_render_scene::hd::kit::look_at;

#[test]
fn the_view_reads_the_cameras_axes_and_pixel_size() {
    let eye = [0.0, -10.0, 2.0];
    let camera = HdCamera::new(look_at(eye, [0.0, 0.0, 2.0]), 90.0_f32, 1000.0, 960, 540);
    let view = StripView::from_matrices(eye, &camera.view, &camera.proj, camera.height);
    let close = |a: Vec3, b: Vec3| a.distance(b) < 1.0e-4;
    assert!(close(view.forward, Vec3::Y), "{:?}", view.forward);
    assert!(close(view.right, Vec3::X), "{:?}", view.right);
    assert!(close(view.up, Vec3::Z), "{:?}", view.up);
    // A 90 degree horizontal field over 960 pixels: a pixel ten units away is 2 * 10 / 960 across.
    let pixel = view.pixel_at(Vec3::new(0.0, 0.0, 2.0));
    assert!((pixel - 20.0 / 960.0).abs() < 1.0e-5, "{pixel}");
}
