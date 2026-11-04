use super::*;
use crate::hd::kit::look_at;

#[test]
fn the_ladder_is_as_documented() {
    assert_eq!(Quality::Low.reflections().water, WaterReflection::Sky);
    assert_eq!(Quality::Medium.reflections().water, WaterReflection::Ssr);
    assert_eq!(Quality::Medium.reflections().ssr, SsrResolution::Off);
    assert_eq!(Quality::High.reflections().ssr, SsrResolution::Half);
    assert_eq!(
        Quality::Ultra.reflections().water,
        WaterReflection::Planar { half: false }
    );
    assert!(Quality::High.reflections().capture && !Quality::Medium.reflections().capture);
}

#[test]
fn the_mirrored_camera_is_the_reflected_eye_upside_down() {
    let eye = look_at([3.0, -9.0, 6.0], [1.0, 4.0, 0.5]);
    let level = 0.75;
    let cam = HdCamera::new(eye, 90u8, 500.0, 640, 360);
    let mirror = mirror_camera(&cam, level);
    let reference = HdCamera::new(reflected_eye(eye, level), 90u8, 500.0, 640, 360);
    let mut seen = 0;
    for p in [
        [1.0, 4.0, 1.5],
        [-2.0, 6.0, 3.0],
        [4.0, 2.0, 0.9],
        [0.0, 12.0, 7.0],
    ] {
        let (Some(a), Some(b)) = (mirror.project(p), reference.project(p)) else {
            continue;
        };
        seen += 1;
        assert!((a.0 - b.0).abs() < 1e-2, "{a:?} {b:?}");
        assert!((a.1 - (360.0 - b.1)).abs() < 1e-2, "{a:?} {b:?}");
        assert!((a.2 - b.2).abs() < 1e-5, "{a:?} {b:?}");
    }
    assert!(seen >= 3);
    // A point on the plane lands where the main camera sees it.
    let q = [1.0, 4.0, level];
    let (a, b) = (mirror.project(q).unwrap(), cam.project(q).unwrap());
    assert!((a.0 - b.0).abs() < 1e-2 && (a.1 - b.1).abs() < 1e-2);
}

#[test]
fn a_cube_face_sees_its_axis_at_its_centre_and_its_layout() {
    let eye = [1.0, 2.0, 3.0];
    for face in 0..6 {
        let cam = cube_face_camera(eye, face, 0.1, 100.0, 256);
        let (fwd, right, up) = cube_face_axes(face);
        let at = |d: [f64; 3]| [0, 1, 2].map(|k| eye[k] + 5.0 * d[k] as f32);
        let c = cam.project(at(fwd)).unwrap();
        assert!((c.0 - 128.0).abs() < 1e-3 && (c.1 - 128.0).abs() < 1e-3);
        let d = |a: [f64; 3], b: [f64; 3]| [0, 1, 2].map(|k| a[k] + 0.5 * b[k]);
        let r = cam.project(at(d(fwd, right))).unwrap();
        assert!(r.0 > 140.0 && (r.1 - 128.0).abs() < 1e-3, "{face} {r:?}");
        let u = cam.project(at(d(fwd, up))).unwrap();
        assert!(u.1 < 116.0 && (u.0 - 128.0).abs() < 1e-3, "{face} {u:?}");
        assert!(cam.project(at(fwd.map(|v| -v))).is_none());
    }
}
