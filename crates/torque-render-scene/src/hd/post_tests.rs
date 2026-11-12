use super::*;

/// The render scale: native is the quality's own share (no upscaling past it), auto also holds the TAA to 1080p's pixels, a
/// fixed share is that share at every quality.
#[test]
fn the_render_scale_is_the_qualitys_the_capped_or_a_fixed_share() {
    let ultra = Quality::Ultra.post();
    let medium = Quality::Medium.post();
    assert_eq!(RenderScale::Native.scale(&ultra, 2560, 1440), 1.0);
    assert_eq!(RenderScale::Native.scale(&medium, 2560, 1440), 0.85);
    assert_eq!(RenderScale::Auto.scale(&ultra, 2560, 1440), 0.75);
    assert_eq!(RenderScale::Auto.scale(&ultra, 1920, 1080), 1.0);
    assert_eq!(RenderScale::Fixed(0.6).scale(&ultra, 2560, 1440), 0.6);
    assert_eq!(RenderScale::Fixed(0.1).scale(&ultra, 2560, 1440), 0.5);
    for s in [
        RenderScale::Native,
        RenderScale::Auto,
        RenderScale::Fixed(0.75),
    ] {
        assert_eq!(RenderScale::parse(&s.name()), Some(s));
    }
    assert_eq!(RenderScale::parse("0.4"), None);
    assert_eq!(RenderScale::parse("inf"), None);
}
use crate::hd::kit::look_at;

#[test]
fn halton_is_the_radical_inverse_and_the_jitter_cycles_in_eight() {
    assert_eq!(halton(1, 2), 0.5);
    assert_eq!(halton(3, 2), 0.75);
    assert!((halton(1, 3) - 1.0 / 3.0).abs() < 1e-7);
    assert!((halton(5, 3) - 7.0 / 9.0).abs() < 1e-7);
    assert_eq!(jitter(0), jitter(8));
    let mut sum = [0.0f32; 2];
    for k in 0..8 {
        let j = jitter(k);
        assert!(j.iter().all(|v| v.abs() < 0.5), "{j:?}");
        sum[0] += j[0];
        sum[1] += j[1];
    }
    // The eight phases are spread about the centre.
    assert!(sum[0].abs() < 0.5 && sum[1].abs() < 0.8, "{sum:?}");
}

#[test]
fn the_jittered_camera_moves_the_image_by_the_offset() {
    let eye = look_at([3.0, -6.0, 4.0], [0.0, 1.0, 0.5]);
    let cam = HdCamera::new(eye, 90u8, 500.0, 640, 360);
    let j = cam.jittered([0.3, -0.25]);
    for p in [[0.0, 1.0, 0.5], [2.0, 4.0, 1.0], [-3.0, 9.0, 0.0]] {
        let (x, y, z) = cam.project(p).unwrap();
        let (jx, jy, jz) = j.project(p).unwrap();
        assert!((jx - x - 0.3).abs() < 1e-3, "{x} {jx}");
        assert!((jy - y + 0.25).abs() < 1e-3, "{y} {jy}");
        assert!((jz - z).abs() < 1e-6);
    }
    let s = cam.sized(320, 180);
    let (x, y, _) = s.project([0.0, 1.0, 0.5]).unwrap();
    let (fx, fy, _) = cam.project([0.0, 1.0, 0.5]).unwrap();
    assert!((x * 2.0 - fx).abs() < 1e-3 && (y * 2.0 - fy).abs() < 1e-3);
}

#[test]
fn the_ladder_and_the_sizes() {
    assert_eq!(Quality::Low.post().aa, AntiAliasing::Fxaa);
    assert_eq!(Quality::Medium.post().ao, AoMethod::Ssao);
    assert!(Quality::Ultra.post().sharpen);
    assert_eq!(internal_size(1920, 1080, 0.85), (1632, 918));
    assert_eq!(internal_size(1280, 720, 1.0), (1280, 720));
    assert_eq!(internal_size(1, 1, 0.5), (1, 1));
    // A TAA frame larger than 1080p is drawn at 1080p's pixels.
    let ultra = Quality::Ultra.post();
    assert_eq!(ultra.render_scale_at(1920, 1080), 1.0);
    assert_eq!(
        internal_size(2560, 1440, ultra.render_scale_at(2560, 1440)),
        (1920, 1080)
    );
    assert_eq!(Quality::Low.post().render_scale_at(2560, 1440), 0.75);
}

#[test]
fn a_moved_shape_points_back_to_where_it_was() {
    let mut cur = Mat::from_axis_angle([0.0, 0.0, 1.0], 0.7);
    for k in 0..3 {
        for c in 0..3 {
            cur.m[k * 4 + c] *= 1.5;
        }
    }
    cur.set_column3([2.0, -1.0, 0.5]);
    let mut prev = Mat::from_axis_angle([0.0, 0.0, 1.0], 0.4);
    prev.set_column3([1.0, 0.0, 0.5]);
    let o = crate::hd::kit::box_object(1, [0.0; 3], [0.5; 3]);
    let o = DynamicObject {
        transform: cur,
        prev_transform: prev,
        ..o
    };
    let m = o.motion();
    let local = [0.3, -0.2, 0.9];
    let now = cur.mul_p(local);
    let then = prev.mul_p(local);
    let got = [0, 1, 2].map(|r| m[r] * now[0] + m[4 + r] * now[1] + m[8 + r] * now[2] + m[12 + r]);
    for k in 0..3 {
        assert!((got[k] - then[k]).abs() < 1e-4, "{got:?} {then:?}");
    }
    let still = DynamicObject {
        prev_transform: cur,
        ..o
    };
    assert_eq!(still.motion(), COLUMN_IDENTITY);
}
