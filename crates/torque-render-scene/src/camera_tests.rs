use super::*;

fn project(vp: &[f64; 16], p: [f64; 3], w: f64, h: f64) -> (f64, f64) {
    let clip: Vec<f64> = (0..4)
        .map(|r| vp[r] * p[0] + vp[4 + r] * p[1] + vp[8 + r] * p[2] + vp[12 + r])
        .collect();
    (
        (clip[0] / clip[3] + 1.0) * 0.5 * w,
        (1.0 - clip[1] / clip[3]) * 0.5 * h,
    )
}

#[test]
fn the_point_looked_at_is_at_the_centre_and_the_fov_is_horizontal() {
    // A camera at 2 -3 1.8 looking at 0 0 0.3.
    let (dx, dy, dz): (f64, f64, f64) = (-2.0, 3.0, -1.5);
    let eye = Eye {
        position: [2.0, -3.0, 1.8],
        pitch: (-dz).atan2((dx * dx + dy * dy).sqrt()) as f32,
        yaw: (-(-dx).atan2(dy)) as f32,
        fov: Some(90),
    };
    let cam = Camera::from_eye(eye, 90u8, 1000.0, 800, 600);
    let vp = cam.view_proj();
    let (x, y) = project(&vp, [0.0, 0.0, 0.3], 800.0, 600.0);
    assert!(
        (x - 400.0).abs() < 1e-3 && (y - 300.0).abs() < 1e-3,
        "{x} {y}"
    );
    // 90 degrees across 800 pixels: a point 45 degrees right of the view axis lands on the right edge.
    let f = cam.transform.column(1);
    let r = cam.transform.column(0);
    let p = [
        2.0 + f64::from(f[0] + r[0]),
        -3.0 + f64::from(f[1] + r[1]),
        1.8 + f64::from(f[2] + r[2]),
    ];
    let (x, _) = project(&vp, p, 800.0, 600.0);
    assert!((x - 800.0).abs() < 1e-3, "{x}");
}
