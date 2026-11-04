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
