use super::*;
use crate::hd::cells::StaticCells;

#[test]
fn a_new_frame_over_plain_cells_draws_nothing_and_waits_for_nothing() {
    let mut scene = HdScene::new(StaticCells::<()>::default());
    assert!(scene.cells.cells().is_empty() && scene.objects.is_empty() && scene.lights.is_empty());
    assert_eq!(scene.fov, DEFAULT_FOV);
    assert!(!scene.pending());
    scene.textures_pending = true;
    assert!(scene.pending());
}

#[test]
fn the_sky_sun_is_the_suns_direction_without_a_painted_one() {
    let mut scene = HdScene::new(StaticCells::<()>::default());
    scene.sun.towards = [0.0, 0.6, 0.8];
    assert_eq!(scene.sky_sun(), [0.0, 0.6, 0.8]);
}

#[test]
fn a_frames_particles_are_unlit_sprites_and_the_additive_ones_glow() {
    use crate::textures::tests::png;
    use crate::{Assets, Cull, TexEnv, Vertex};
    struct One;
    impl Assets for One {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            (path == "fx/spark.png").then(|| png(4, 4, 200))
        }
    }
    let quad = |blend: Blend, texture: Option<usize>| Draw {
        label: "particles".to_string(),
        vertices: vec![
            Vertex {
                position: [0.0; 3],
                normal: [0.0, 0.0, 1.0],
                color: [255, 160, 40, 255],
                uv: [0.0; 2],
            };
            6
        ],
        lighting: false,
        tex_env: if texture.is_some() {
            TexEnv::Modulate
        } else {
            TexEnv::None
        },
        texture: texture.map(|t| {
            (
                t,
                crate::Sampler {
                    repeat: false,
                    mag: crate::Filter::Linear,
                    min: crate::Filter::Linear,
                    mip: crate::Filter::Linear,
                    mipmaps: true,
                },
            )
        }),
        blend,
        cull: Cull::None,
        depth_test: true,
        depth_write: false,
        fog: false,
        lights: Vec::new(),
        vertex_key: None,
    };
    let mut scene = HdScene::new(StaticCells::<()>::default());
    // A texture the table held before: the particles' set's first is the table's second.
    scene.textures.intern(crate::Texture {
        path: "other.png".to_string(),
        width: 1,
        height: 1,
        levels: std::sync::Arc::new(vec![vec![0; 4]]),
    });
    let mut set = TextureSet::new(&One);
    let spark = set.get_modulated("fx/spark.png").expect("the spark loads");
    let draws = [
        (4.0, quad(Blend::Additive, Some(spark))),
        (9.0, quad(Blend::Alpha, None)),
    ];
    scene.set_particles(&draws, set);
    assert_eq!(scene.particles_generation, 1);
    assert_eq!(scene.particles.len(), 2);
    let (dist, glow) = &scene.particles[0];
    assert_eq!(*dist, 4.0);
    assert_eq!(glow.texture.map(|t| t.0), Some(1), "the table's index");
    let word = glow.vertices[0].material;
    assert_eq!((word >> material::KIND_SHIFT) & 3, material::KIND_SKY);
    assert_ne!(word & material::PARTICLE, 0);
    assert_ne!(word & material::UNLIT, 0);
    assert_ne!(word & material::EMITTER, 0, "an additive particle glows");
    let word = scene.particles[1].1.vertices[0].material;
    assert_ne!(word & material::PARTICLE, 0);
    assert_eq!(word & material::EMITTER, 0, "an alpha particle does not");
    // The next frame's set replaces this one.
    scene.set_particles(&[], TextureSet::new(&One));
    assert!(scene.particles.is_empty());
    assert_eq!(scene.particles_generation, 2);
}

#[test]
fn a_spot_lights_its_cone_with_a_soft_edge() {
    let spot = HdSpot {
        light: HdLight {
            position: [0.0, 0.0, 5.0],
            color: [1.0; 3],
            radius: 20.0,
            carrier: None,
        },
        direction: [0.0, 0.0, -2.0],
        inner: 20f32.to_radians(),
        outer: 30f32.to_radians(),
        shadows: true,
    };
    let at = |deg: f32| {
        let r = 5.0 * deg.to_radians().tan();
        spot.share([r, 0.0, 0.0])
    };
    assert_eq!(at(0.0), 1.0);
    assert_eq!(at(19.0), 1.0);
    assert_eq!(at(31.0), 0.0);
    let edge = at(25.0);
    assert!(edge > 0.2 && edge < 0.8, "{edge}");
    assert!(at(22.0) > at(25.0) && at(25.0) > at(28.0));
    // A point light's packed cone (all zeros) lights everything.
    assert_eq!(cone_share([0.0; 4], [0.0; 3], [1.0, 2.0, 3.0]), 1.0);
    // Behind the light: none.
    assert_eq!(spot.share([0.0, 0.0, 9.0]), 0.0);
}

#[test]
fn a_spot_of_numbers_that_are_not_finite_packs_a_finite_cone() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let spot = HdSpot {
            light: HdLight {
                position: [0.0; 3],
                color: [1.0; 3],
                radius: 10.0,
                carrier: None,
            },
            direction: [bad, 0.0, 1.0],
            inner: bad,
            outer: bad,
            shadows: false,
        };
        assert!(spot.cone().iter().all(|v| v.is_finite()), "{bad}");
    }
}
