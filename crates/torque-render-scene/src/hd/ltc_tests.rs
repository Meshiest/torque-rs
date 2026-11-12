use super::*;

/// The lobe's albedo (F = 1) and Schlick-weighted part by a plain quadrature over the hemisphere (`n` x `n` cells in cos theta
/// and phi): another estimate than the fit's importance-sampled moments.
fn quadrature(rough: f64, nv: f64, n: usize) -> (f64, f64) {
    let a = alpha_of(rough);
    let v = dvec3((1.0 - nv * nv).max(0.0).sqrt(), 0.0, nv);
    let (mut albedo, mut fresnel) = (0.0, 0.0);
    for (u1, u2) in strata(n) {
        let (cos_t, phi) = (u2, 2.0 * PI * u1);
        let sin_t = (1.0 - cos_t * cos_t).sqrt();
        let l = dvec3(sin_t * phi.cos(), sin_t * phi.sin(), cos_t);
        let f = lobe(v, l, a);
        let vh = v.dot((v + l).normalize()).clamp(0.0, 1.0);
        albedo += f;
        fresnel += f * (1.0 - vh).powi(5);
    }
    let w = 2.0 * PI / (n * n) as f64;
    (albedo * w, fresnel * w)
}

/// The half L1 distance between the cell's normalized lobe and its fitted distribution (0 a perfect fit, 1 none in common),
/// each estimated from directions drawn by its own density.
fn misfit(i: usize, j: usize) -> f64 {
    let (rough, nv) = cell_params(i, j);
    let a = alpha_of(rough);
    let v = dvec3((1.0 - nv * nv).max(0.0).sqrt(), 0.0, nv);
    let c = table()[i * LTC_SIZE + j];
    let [ma, mb, mc, md] = c.minv.map(f64::from);
    let minv = DMat3::from_cols(dvec3(ma, 0.0, mc), DVec3::Y, dvec3(mb, 0.0, md));
    let ltc = Ltc {
        m: minv.inverse(),
        minv,
        det_inv: minv.determinant().abs(),
    };
    let albedo = f64::from(c.albedo);
    let n = 64;
    // E_f[max(0, 1 - D / f)]: the lobe's mass the distribution does not cover.
    let missed: f64 = strata(n)
        .map(|(u1, u2)| {
            let l = lobe_sample(v, a, u1, u2);
            let f = lobe(v, l, a) / albedo;
            let pdf = lobe_pdf(v, l, a);
            if f <= 0.0 || pdf <= 0.0 {
                return 0.0;
            }
            (f - ltc.density(l)).max(0.0) / pdf
        })
        .sum();
    missed / (n * n) as f64
}

#[test]
fn table_is_whole_and_finite() {
    let t = table();
    assert_eq!(t.len(), LTC_SIZE * LTC_SIZE);
    for (k, c) in t.iter().enumerate() {
        assert!(
            c.minv.iter().all(|v| v.is_finite()) && c.albedo.is_finite() && c.fresnel.is_finite(),
            "cell {k}: {c:?}"
        );
        assert!(
            c.albedo > 0.0 && c.albedo <= 1.0 && (0.0..=c.albedo).contains(&c.fresnel),
            "cell {k}: {c:?}"
        );
        // Half floats hold it (the GPU's table).
        assert!(c.minv.iter().all(|v| v.abs() < 6e4), "cell {k}: {c:?}");
    }
}

#[test]
fn table_is_the_fit() {
    // A row's first cells fitted again here are the committed ones (the fit is deterministic; libm's last bits aside).
    for i in [0, 21, 63] {
        let mut start = None;
        for j in 0..3 {
            let (fit, cell) = fit_cell(i, j, start);
            start = Some(fit);
            let kept = table()[i * LTC_SIZE + j];
            for (a, b) in cell.minv.iter().zip(kept.minv) {
                assert!(
                    (a - b).abs() <= 1e-4 * b.abs().max(1.0),
                    "({i}, {j}): {cell:?} vs {kept:?}"
                );
            }
            assert!((cell.albedo - kept.albedo).abs() < 1e-5, "({i}, {j})");
        }
    }
}

#[test]
fn moments_are_the_quadratures() {
    // The table's albedo and Schlick part against a plain quadrature (the lobes rough enough for its cells).
    for i in (24..LTC_SIZE).step_by(13) {
        for j in (0..LTC_SIZE - 4).step_by(12) {
            let (rough, nv) = cell_params(i, j);
            let (albedo, fresnel) = quadrature(rough, nv, 400);
            let c = table()[i * LTC_SIZE + j];
            assert!(
                (f64::from(c.albedo) - albedo).abs() < 0.01,
                "({i}, {j}): {} vs {albedo}",
                c.albedo
            );
            assert!(
                (f64::from(c.fresnel) - fresnel).abs() < 0.005,
                "({i}, {j}): {} vs {fresnel}",
                c.fresnel
            );
        }
    }
}

#[test]
fn fits_cover_the_lobe() {
    // Most of each lobe's mass lies under its fitted distribution away from the most grazing views.
    let mut worst = (0.0, 0, 0);
    for i in (4..LTC_SIZE).step_by(6) {
        for j in (0..LTC_SIZE - 6).step_by(6) {
            let m = misfit(i, j);
            if m > worst.0 {
                worst = (m, i, j);
            }
        }
    }
    assert!(worst.0 < 0.25, "worst misfit {worst:?}");
}

#[test]
fn lookup_at_a_center_is_its_cell() {
    let last = (LTC_SIZE - 1) as f32;
    for (i, j) in [(0, 0), (10, 40), (63, 63), (32, 7)] {
        let (rough, nv) = cell_params(i, j);
        let s = j as f32 / last;
        let nv_cell = if j == LTC_SIZE - 1 {
            1.0 - s * s
        } else {
            nv as f32
        };
        let got = lookup(rough as f32, nv_cell);
        let want = table()[i * LTC_SIZE + j];
        for (a, b) in got.minv.iter().zip(want.minv) {
            assert!(
                (a - b).abs() <= 1e-3 * b.abs().max(1.0),
                "({i}, {j}): {got:?} vs {want:?}"
            );
        }
    }
}
