use super::*;

#[test]
fn the_transmittance_mapping_round_trips() {
    let a = EARTH;
    for (u, v) in [(0.1, 0.2), (0.5, 0.5), (0.93, 0.07), (0.02, 0.98)] {
        let (r, mu) = a.transmittance_r_mu(u, v);
        let (u2, v2) = a.transmittance_uv(r, mu);
        assert!(
            (u - u2).abs() < 1e-9 && (v - v2).abs() < 1e-9,
            "{u} {v} {u2} {v2}"
        );
    }
}

#[test]
fn the_sky_view_mapping_round_trips_and_crowds_the_horizon() {
    let a = EARTH;
    let r = a.bottom + CAMERA_HEIGHT;
    for (u, v) in [(0.1, 0.1), (0.5, 0.49), (0.7, 0.51), (0.9, 0.95)] {
        let (az, el) = sky_view_angles(&a, r, u, v);
        let (u2, v2) = sky_view_uv(&a, r, az, el);
        assert!(
            (u - u2).abs() < 1e-9 && (v - v2).abs() < 1e-9,
            "{u} {v} {u2} {v2}"
        );
    }
    let (_, near) = sky_view_angles(&a, r, 0.5, 0.45);
    let (_, far) = sky_view_angles(&a, r, 0.5, 0.05);
    assert!(near.abs() < 0.05 && far > 1.0, "{near} {far}");
}

#[test]
fn the_transmittance_is_bluer_lost_at_the_horizon() {
    let a = EARTH;
    let r = a.bottom + CAMERA_HEIGHT;
    let zenith = a.transmittance_direct(r, 1.0, 512);
    let horizon = a.transmittance_direct(r, 0.01, 2048);
    assert!(zenith.iter().all(|t| *t > 0.6 && *t < 1.0), "{zenith:?}");
    assert!(
        horizon[2] < horizon[0] && horizon[0] < zenith[0],
        "{horizon:?}"
    );
}

#[test]
fn the_phases_integrate_to_one() {
    let n = 20_000;
    let (mut r, mut m) = (0.0, 0.0);
    for i in 0..n {
        let nu = -1.0 + 2.0 * (i as f64 + 0.5) / n as f64;
        r += rayleigh_phase(nu) * 2.0 * std::f64::consts::PI * 2.0 / n as f64;
        m += mie_phase(nu, 0.8) * 2.0 * std::f64::consts::PI * 2.0 / n as f64;
    }
    assert!((r - 1.0).abs() < 1e-6 && (m - 1.0).abs() < 1e-3, "{r} {m}");
}

#[test]
fn directions_have_their_azimuth_and_elevation() {
    let sun = DVec3::new(0.3, -0.8, 0.4).normalize().to_array();
    let d = direction(sun, 0.0, 0.0);
    let h = (sun[0] * sun[0] + sun[1] * sun[1]).sqrt();
    assert!((d[0] - sun[0] / h).abs() < 1e-12 && d[2].abs() < 1e-12);
    let d = direction(sun, 1.0, 0.5);
    assert!((d[2] - 0.5f64.sin()).abs() < 1e-12);
    let dh = [d[0], d[1]];
    let c = (dh[0] * sun[0] + dh[1] * sun[1]) / (dh[0].hypot(dh[1]) * h);
    assert!((c - 1.0f64.cos()).abs() < 1e-12);
}

#[test]
fn the_gate_texels_are_256_distinct() {
    let mut t = gate_texels();
    t.sort_unstable();
    t.dedup();
    assert_eq!(t.len(), 256);
}
