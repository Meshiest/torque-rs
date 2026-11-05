use super::*;
use crate::hd::kit::Lcg;

/// Exposed colours from black to far past white: greys, saturated and mixed colours.
fn colours(n: usize) -> Vec<[f32; 3]> {
    let mut rng = Lcg::new(7);
    let mut out = Vec::new();
    for i in 0..n {
        let ev = |u: f32| (-16.0 + u * 24.0).exp2();
        let c = match i % 4 {
            0 => [ev(rng.unit()); 3],
            1 => [ev(rng.unit()), ev(rng.unit()) * 0.01, ev(rng.unit()) * 0.05],
            _ => [ev(rng.unit()), ev(rng.unit()), ev(rng.unit())],
        };
        out.push(c);
    }
    out.push([0.0; 3]);
    out
}

#[test]
fn the_identity_grade_table_is_the_curve_within_half_a_display_step() {
    {
        let settings = crate::hd::ImageSettings::DEFAULT;
        let table = bake(settings, &Grade::NEUTRAL, None);
        let mut worst = 0.0f32;
        for c in colours(40_000) {
            let a = curve(settings, c);
            let b = lookup(settings, &Grade::NEUTRAL, &table, c);
            for k in 0..3 {
                worst = worst.max((a[k] - b[k]).abs());
            }
        }
        assert!(worst * 255.0 <= 0.5, "{:.3}/255", worst * 255.0);
    }
}

#[test]
fn the_graded_table_is_the_exact_formation_within_a_display_step() {
    {
        let settings = crate::hd::ImageSettings::DEFAULT;
        let g = settings.grade();
        let table = bake(settings, &g, None);
        let mut worst = 0.0f32;
        for c in colours(10_000) {
            let a = formed(settings, &g, None, c);
            let b = lookup(settings, &g, &table, c);
            for k in 0..3 {
                worst = worst.max((a[k] - b[k]).abs());
            }
        }
        assert!(worst * 255.0 <= 1.0, "{:.3}/255", worst * 255.0);
    }
}

#[test]
fn an_identity_cube_changes_nothing() {
    let id = CubeLut::identity(17);
    {
        let settings = crate::hd::ImageSettings::DEFAULT;
        let g = settings.grade();
        for c in colours(2000) {
            let a = formed(settings, &g, None, c);
            let b = formed(settings, &g, Some(&id), c);
            for k in 0..3 {
                assert!((a[k] - b[k]).abs() < 1e-5);
            }
        }
    }
}

#[test]
fn a_cube_file_reads_and_hostile_ones_are_refused() {
    let mut ok = String::from(
        "# a comment\nTITLE \"warm\"\nLUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n",
    );
    for b in 0..2 {
        for g in 0..2 {
            for r in 0..2 {
                ok.push_str(&format!("{r} {g} {}\n", 0.9 * b as f32));
            }
        }
    }
    let lut = parse_cube(&ok).unwrap();
    assert_eq!((lut.size, lut.title.as_str()), (2, "warm"));
    let s = lut.sample([1.0, 1.0, 1.0]);
    assert!((s[2] - 0.9).abs() < 1e-6);
    let rows8 = "0 0 0\n".repeat(8);
    let rows7 = "0 0 0\n".repeat(7);
    let hostile = [
        String::new(),
        "LUT_3D_SIZE 2\n0 0 0\n".to_string(),
        format!("LUT_3D_SIZE 2\n{rows8}0 0 0\n"),
        format!("LUT_3D_SIZE 100000\n{rows8}"),
        format!("LUT_3D_SIZE 1\n{rows8}"),
        format!("LUT_3D_SIZE -3\n{rows8}"),
        format!("LUT_3D_SIZE 2\nLUT_3D_SIZE 2\n{rows8}"),
        format!("LUT_3D_SIZE 2\n{rows7}NaN 0 0\n"),
        format!("LUT_3D_SIZE 2\n{rows7}inf 0 0\n"),
        format!("LUT_3D_SIZE 2\n{rows7}1e30 0 0\n"),
        format!("LUT_3D_SIZE 2\n{rows7}0 0\n"),
        format!("LUT_3D_SIZE 2\n{rows7}0 0 0 0\n"),
        format!("LUT_3D_SIZE 2\n{rows8}TITLE \"late\"\n"),
        format!("0 0 0\nLUT_3D_SIZE 2\n{rows7}"),
        "LUT_1D_SIZE 2\n0 0 0\n0 0 0\n".to_string(),
        format!("LUT_3D_SIZE 2\nDOMAIN_MIN 1 0 0\nDOMAIN_MAX 1 1 1\n{rows8}"),
        format!("LUT_3D_SIZE 2\nSHELL rm\n{rows8}"),
        format!("LUT_3D_SIZE 2\n{rows7}0 0 {}\n", "1".repeat(600)),
        format!("LUT_3D_SIZE 2\n{rows7}0\u{0} 0 0\n"),
        format!("LUT_3D_SIZE 2\n{rows7}0x10 0 0\n"),
    ];
    for (i, h) in hostile.iter().enumerate() {
        assert!(parse_cube(h).is_err(), "hostile file {i} was read");
    }
    // A text over the cap is refused before it is parsed.
    let big = "#".repeat(MAX_CUBE_BYTES as usize + 1);
    assert!(parse_cube(&big).is_err());
}

#[test]
fn the_default_grade_stays_near_range_and_keep_black_and_white() {
    {
        let settings = crate::hd::ImageSettings::DEFAULT;
        let g = settings.grade();
        let t = bake(settings, &g, None);
        assert_eq!(t.len(), LUT_SIZE.pow(3));
        assert!(
            t.iter()
                .all(|c| c.iter().all(|v| v.is_finite() && *v >= -0.5 && *v <= 1.5))
        );
        let black = formed(settings, &g, None, [0.0; 3]);
        assert!(black.iter().all(|v| *v < 0.02), "{black:?}");
        let white = formed(settings, &g, None, [1e4; 3]);
        assert!(white.iter().all(|v| *v > 0.95), "{white:?}");
    }
}
