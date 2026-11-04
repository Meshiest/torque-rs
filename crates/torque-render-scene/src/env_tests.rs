use super::*;
use crate::tge::tests::tge_world;

#[test]
fn a_skys_distances_and_fog_colour_and_the_suns_light_make_the_environment() {
    let w = tge_world([800.0, 400.0], 90.0, 0.0);
    let e = Environment::of(&w);
    assert_eq!((e.visible_distance, e.fog_distance), (800.0, 400.0));
    assert_eq!(e.clear, [0.5, 0.6, 0.7]);
    assert!((e.sun.towards[0] - 1.0).abs() < 1e-5 && e.sun.towards[2].abs() < 1e-5);
    assert_eq!(e.sun.diffuse, [0.6; 3]);
    assert_eq!(e.sun.ambient, [0.3; 3]);
    assert_eq!(e.fog, None);
    assert_eq!(sky_visible(&w), Some(800.0));
    assert_eq!(sky_material(&w), Some(String::new()));
    // Without a Sky: the scene graph's defaults and a white clear.
    let none = ghost_world::world::ClientWorld::<ghost_world::tge::TgeClientSet>::default();
    let e = Environment::of(&none);
    assert_eq!(
        (e.visible_distance, e.fog_distance, e.clear),
        (500.0, 250.0, [1.0; 3])
    );
}

#[test]
fn a_view_distance_cap_under_the_skys_scales_the_fog_with_it() {
    assert_eq!(capped(800.0, 400.0, 1000.0), (800.0, 400.0));
    assert_eq!(capped(800.0, 400.0, 800.0), (800.0, 400.0));
    assert_eq!(capped(800.0, 400.0, 200.0), (200.0, 100.0));
    assert_eq!(capped(1000.0, 0.0, 50.0), (50.0, 0.0));
    assert_eq!(capped(800.0, 400.0, f32::NAN), (800.0, 400.0));
    let mut w = tge_world([800.0, 400.0], 0.0, 45.0);
    w.max_view_distance = Some(200.0);
    let e = Environment::of(&w);
    assert_eq!((e.visible_distance, e.fog_distance), (200.0, 100.0));
}

#[test]
fn an_overlays_frame_is_the_cameras_with_the_environments_clear_and_no_draws() {
    let mut w = tge_world([800.0, 400.0], 0.0, 45.0);
    assert!(
        overlay_scene(&w, 90.0, (640, 480)).is_none(),
        "no camera yet"
    );
    w.view = Some(crate::source::Eye {
        position: [1.0, 2.0, 3.0],
        pitch: 0.0,
        yaw: 0.0,
        fov: Some(90),
    });
    let s = overlay_scene(&w, 90.0, (640, 480)).unwrap();
    assert_eq!((s.width, s.height), (640, 480));
    assert_eq!(s.camera.eye, [1.0, 2.0, 3.0]);
    assert_eq!(s.clear, [0.5, 0.6, 0.7]);
    assert!(s.draws.is_empty() && s.fog.is_none());
}
