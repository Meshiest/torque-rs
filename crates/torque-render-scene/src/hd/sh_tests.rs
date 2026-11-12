use super::*;
use crate::{Cull, Vertex};

#[test]
fn the_uniform_shape_is_one_everywhere() {
    for n in [0, 100, 777] {
        let v = eval(&uniform(), direction(n));
        assert!(v.iter().all(|x| (x - 1.0).abs() < 1e-5), "{v:?}");
    }
}

#[test]
fn a_sky_brighter_above_lights_up_facing_normals_more() {
    // A white dome above the horizon over a dark clear: the irradiance from above beats the one from below.
    let mut v = Vec::new();
    let r = 100.0;
    for k in 0..16 {
        let a0 = k as f32 * std::f32::consts::TAU / 16.0;
        let a1 = (k + 1) as f32 * std::f32::consts::TAU / 16.0;
        let p = |a: f32, z: f32| Vertex {
            position: [r * a.cos(), r * a.sin(), z],
            normal: [0.0, 0.0, 1.0],
            color: [255; 4],
            uv: [0.0; 2],
        };
        let mut apex = p(0.0, r);
        apex.position = [0.0, 0.0, r];
        v.extend([p(a0, 0.0), p(a1, 0.0), apex]);
    }
    let draw = Draw {
        label: "sky face 4".into(),
        vertices: v,
        lighting: false,
        tex_env: TexEnv::None,
        texture: None,
        blend: Blend::Opaque,
        cull: Cull::None,
        depth_test: false,
        depth_write: false,
        fog: false,
        lights: Vec::new(),
        vertex_key: None,
    };
    let c = sky_shape(
        &[draw],
        &TextureTable::default(),
        &[],
        [0.0; 3],
        [0.1, 0.1, 0.1],
    );
    let up = eval(&c, [0.0, 0.0, 1.0])[1];
    let down = eval(&c, [0.0, 0.0, -1.0])[1];
    let side = eval(&c, [1.0, 0.0, 0.0])[1];
    assert!(up > side && side > down, "{up} {side} {down}");
    assert!((side - 1.0).abs() < 0.05, "{side}");
}

#[test]
#[ignore = "half a minute unoptimized: the atmosphere's reference integrator builds its tables"]
fn the_physical_skys_light_at_noon_is_near_the_days_ambient() {
    // The drawn sky (the reference integrator's, times the gain) for a sun overhead, as light by `sky_light_scale`, on a level
    // face: the atmosphere's own ambient for the sun of `NOON_SUN`. The day's light (`scatter`) sets its noon ambient to
    // `NOON_AMBIENT` by a scale of its own; the two calibrations agree within a third.
    use crate::hd::atmosphere::{CAMERA_HEIGHT, EARTH, Reference};
    let reference = Reference::new(EARTH);
    let r = EARTH.bottom + CAMERA_HEIGHT;
    // (The sky about a sun overhead is the same at every azimuth.)
    let (bands, azimuths) = (16, 1);
    let mut e = 0.0f64;
    for b in 0..bands {
        let el = (b as f64 + 0.5) / bands as f64 * std::f64::consts::FRAC_PI_2;
        let (se, ce) = el.sin_cos();
        for a in 0..azimuths {
            let phi = (a as f64 + 0.5) / azimuths as f64 * std::f64::consts::TAU;
            let l = reference.sky(r, [phi.cos() * ce, phi.sin() * ce, se], [0.0, 0.0, 1.0]);
            let lum = 0.2126 * l[0] + 0.7152 * l[1] + 0.0722 * l[2];
            let w = se
                * ce
                * (std::f64::consts::FRAC_PI_2 / bands as f64)
                * (std::f64::consts::TAU / azimuths as f64);
            e += lum * w;
        }
    }
    let drawn = e * f64::from(crate::hd::sky::DEFAULT_SKY_GAIN);
    let ambient = drawn * f64::from(sky_light_scale()) / std::f64::consts::PI;
    let day = f64::from(crate::hd::scatter::NOON_AMBIENT);
    println!(
        "noon: the atmosphere's ambient {ambient:.4}, the day's {day:.4}, scale {}",
        sky_light_scale()
    );
    assert!(
        (ambient / day - 1.0).abs() < 1.0 / 3.0,
        "{ambient} against {day}"
    );
}
