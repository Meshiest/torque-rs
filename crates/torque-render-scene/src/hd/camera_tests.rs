use super::*;

#[test]
fn the_reversed_camera_puts_the_same_pixels_where_the_fixed_function_one_does() {
    let (dx, dy, dz): (f64, f64, f64) = (-2.0, 3.0, -1.5);
    let eye = Eye {
        position: [2.0, -3.0, 1.8],
        pitch: (-dz).atan2((dx * dx + dy * dy).sqrt()) as f32,
        yaw: (-(-dx).atan2(dy)) as f32,
        fov: Some(90),
    };
    let fixed = Camera::from_eye(eye, 90u8, 1000.0, 800, 600);
    let hd = HdCamera::of(&fixed, 800, 600);
    let vp = fixed.view_proj();
    for p in [[0.0, 0.0, 0.3], [1.0, 2.0, 0.0], [-5.0, 9.0, 2.0]] {
        let c: Vec<f64> = (0..4)
            .map(|r| vp[r] * p[0] + vp[4 + r] * p[1] + vp[8 + r] * p[2] + vp[12 + r])
            .collect();
        let (fx, fy) = ((c[0] / c[3] + 1.0) * 400.0, (1.0 - c[1] / c[3]) * 300.0);
        let (x, y, z) = hd.project([p[0] as f32, p[1] as f32, p[2] as f32]).unwrap();
        assert!((f64::from(x) - fx).abs() < 1e-2 && (f64::from(y) - fy).abs() < 1e-2);
        assert!(z > 0.0 && z < 1.0);
    }
    // Nearer is deeper: reversed depth.
    let (_, _, a) = hd.project([0.0, 0.0, 0.3]).unwrap();
    let (_, _, b) = hd.project([-2.0, 3.0, -1.2]).unwrap();
    assert!(a > b, "{a} {b}");
    assert!(hd.sees_box([-1.0, -1.0, 0.0], [1.0, 1.0, 1.0]));
    assert!(!hd.sees_box([50.0, -60.0, 0.0], [51.0, -59.0, 1.0]));
}
