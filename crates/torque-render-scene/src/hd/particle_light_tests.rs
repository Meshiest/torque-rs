use super::*;
use crate::hd::cells::StaticCells;
use crate::hd::{HdScene, oct_decode};
use crate::textures::TextureSet;
use crate::{Assets, Cull, TexEnv, Vertex};

struct NoFiles;
impl Assets for NoFiles {
    fn read(&self, _: &str) -> Option<Vec<u8>> {
        None
    }
}

fn vertex(position: [f32; 3]) -> Vertex {
    Vertex {
        position,
        normal: [0.0, 0.0, 1.0],
        color: [200, 200, 200, 255],
        uv: [0.0; 2],
    }
}

/// Sprites of half size `w` at `centers` facing -y, two triangles each as the emitters lay them out.
fn sprites(blend: Blend, centers: &[[f32; 3]], w: f32) -> Draw {
    let mut vertices = Vec::new();
    for c in centers {
        let at = |x: f32, z: f32| vertex([c[0] + x * w, c[1], c[2] + z * w]);
        let v = [at(-1.0, -1.0), at(1.0, -1.0), at(1.0, 1.0), at(-1.0, 1.0)];
        vertices.extend([v[0], v[1], v[3], v[1], v[2], v[3]]);
    }
    Draw {
        label: "smoke".to_string(),
        vertices,
        lighting: false,
        tex_env: TexEnv::None,
        texture: None,
        blend,
        cull: Cull::None,
        depth_test: true,
        depth_write: false,
        fog: false,
        lights: Vec::new(),
        vertex_key: None,
    }
}

fn lit(scene: &HdScene<StaticCells<()>>, i: usize) -> bool {
    scene.particles[i]
        .1
        .vertices
        .iter()
        .all(|v| v.material & material::LIT != 0)
}

#[test]
fn particles_stay_unlit_without_a_rule_and_the_usual_rule_lights_only_the_blended() {
    let draws = [
        (3.0, sprites(Blend::Alpha, &[[0.0; 3]], 1.0)),
        (4.0, sprites(Blend::Additive, &[[0.0; 3]], 1.0)),
    ];
    let mut scene = HdScene::new(StaticCells::<()>::default());
    scene.set_particles(&draws, TextureSet::new(&NoFiles));
    assert!(!lit(&scene, 0) && !lit(&scene, 1));
    scene.particle_light = Some(ParticleLight::BLENDED);
    scene.set_particles(&draws, TextureSet::new(&NoFiles));
    assert!(lit(&scene, 0), "smoke is lit");
    assert!(!lit(&scene, 1), "a glow is not");
    // A game's own rule: by the draw's label.
    scene.particle_light = Some(ParticleLight(|d| d.label == "spark"));
    scene.set_particles(&draws, TextureSet::new(&NoFiles));
    assert!(!lit(&scene, 0));
}

#[test]
fn a_lit_sprites_normals_point_from_its_center_and_its_extra_is_the_half_diagonal() {
    let centers = [[1.0, 2.0, 3.0], [-4.0, 0.5, 9.0]];
    let d = sprites(Blend::Alpha, &centers, 0.5);
    let mut hd = HdDraw::from_fixed(&d, material::KIND_SKY, &[]);
    assert!(shape_sprites(&mut hd));
    for (quad, c) in hd.vertices.as_chunks::<6>().0.iter().zip(centers) {
        for v in quad {
            let half = f32::from_bits(v.extra);
            assert!((half - 0.5 * 2f32.sqrt()).abs() < 1e-5, "{half}");
            // The center back from the corner along its normal (the shader's step).
            let n = oct_decode(v.normal);
            for k in 0..3 {
                assert!((v.position[k] - n[k] * half - c[k]).abs() < 1e-3);
            }
            assert_ne!(v.material & material::LIT, 0);
        }
    }
}

#[test]
fn a_draw_that_is_not_quads_is_left_unlit() {
    let mut strip = sprites(Blend::Alpha, &[[0.0; 3]], 1.0);
    strip.vertices[5] = vertex([7.0, 0.0, 0.0]);
    let mut hd = HdDraw::from_fixed(&strip, material::KIND_SKY, &[]);
    let before = hd.clone();
    assert!(!shape_sprites(&mut hd));
    assert_eq!(hd, before);
    let mut short = HdDraw::from_fixed(&strip, material::KIND_SKY, &[]);
    short.vertices.truncate(3);
    assert!(!shape_sprites(&mut short));
}
