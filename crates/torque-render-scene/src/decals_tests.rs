use super::*;
use crate::textures::tests::png;
use crate::world::tests::datablock;

struct Files(std::collections::HashMap<String, Vec<u8>>);

impl crate::Assets for Files {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.get(path).cloned()
    }
}

fn hole() -> DecalData {
    DecalData {
        size: [0.2, 0.1],
        texture: Some("kit/hole".to_string()),
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[test]
fn a_decal_lies_in_the_surfaces_plane_lifted_off_it() {
    let mut m = DecalManager::default();
    for (normal, angle) in [
        ([0.0, -1.0, 0.0], 0.0),
        ([0.0, 0.0, 1.0], 0.7),
        ([0.6, 0.0, 0.8], 2.0),
    ] {
        m.clear();
        m.add(&hole(), [1.0, 2.0, 3.0], normal, angle);
        let d = m.decals().next().unwrap();
        let n = unit(normal).unwrap();
        for k in 0..3 {
            assert!((d.position[k] - ([1.0, 2.0, 3.0][k] + n[k] * DECAL_LIFT)).abs() < 1e-5);
        }
        assert!(dot(d.half_x, n).abs() < 1e-5 && dot(d.half_y, n).abs() < 1e-5);
        assert!(dot(d.half_x, d.half_y).abs() < 1e-5);
        let len = |v: [f32; 3]| dot(v, v).sqrt();
        assert!((len(d.half_x) - 0.1).abs() < 1e-5 && (len(d.half_y) - 0.05).abs() < 1e-5);
    }
    // No normal, no decal.
    m.clear();
    m.add(&hole(), [0.0; 3], [0.0; 3], 0.0);
    m.add(&hole(), [f32::NAN, 0.0, 0.0], [0.0, 0.0, 1.0], 0.0);
    assert!(m.is_empty());
}

#[test]
fn decals_fade_at_the_end_of_their_lifetime_and_the_oldest_goes_past_the_cap() {
    let mut m = DecalManager::new(3, 10.0, 2.0);
    for i in 0..5 {
        m.add(&hole(), [i as f32, 0.0, 0.0], [0.0, 0.0, 1.0], 0.0);
        m.advance(1.0);
    }
    assert_eq!(m.len(), 3);
    let xs: Vec<f32> = m.decals().map(|d| d.position[0]).collect();
    assert_eq!(xs, [2.0, 3.0, 4.0], "the oldest went first");
    let first = m.decals().next().unwrap().clone();
    assert_eq!(first.age, 3.0);
    assert_eq!(m.opacity(&first), 1.0);
    m.advance(6.0);
    let first = m.decals().next().unwrap().clone();
    assert!(
        (m.opacity(&first) - 0.5).abs() < 1e-5,
        "halfway through its fade"
    );
    m.advance(1.0);
    assert_eq!(m.len(), 2, "past its lifetime");
}

#[test]
fn a_decal_draws_as_an_alpha_blended_square_without_depth_writes() {
    let mut files = std::collections::HashMap::new();
    files.insert("kit/hole.png".to_string(), png(8, 8, 30));
    let files = Files(files);
    let mut set = TextureSet::new(&files);
    let mut m = DecalManager::new(8, 10.0, 2.0);
    m.add(&hole(), [0.0, 5.0, 1.0], [0.0, -1.0, 0.0], 0.0);
    m.add(
        &DecalData {
            texture: Some("kit/missing".to_string()),
            ..hole()
        },
        [0.0, 5.0, 2.0],
        [0.0, -1.0, 0.0],
        0.0,
    );
    m.advance(9.0);
    let draws = m.draws([0.0, 0.0, 1.0], &mut set);
    assert_eq!(draws.len(), 1, "the one whose texture loads");
    let (dist, d) = &draws[0];
    assert!((dist - (5.0 - DECAL_LIFT)).abs() < 1e-4);
    assert_eq!(d.blend, Blend::Alpha);
    assert!(d.depth_test && !d.depth_write);
    assert_eq!(d.vertices.len(), 6);
    assert!(d.vertices.iter().all(|v| v.color == [255, 255, 255, 128]));
}

#[test]
fn a_projectiles_decal_is_one_of_those_it_names() {
    let mut w = ClientWorld::<TgeClientSet>::default();
    w.datablocks.insert(
        5,
        datablock(
            "DecalData",
            &[
                ("sizeX", Value::F32(0.15)),
                ("textureName", Value::Str(b"kit\\hole".to_vec())),
            ],
        ),
    );
    w.datablocks.insert(
        7,
        datablock("ProjectileData", &[("decals", Value::Id(Some(5)))]),
    );
    w.datablocks.insert(8, datablock("ProjectileData", &[]));
    let mut rng = Rng::new(1);
    assert_eq!(
        projectile_decal(&w, 7, &mut rng),
        Some(DecalData {
            size: [0.15, 1.0],
            texture: Some("kit/hole".to_string()),
        })
    );
    assert_eq!(projectile_decal(&w, 8, &mut rng), None);
}
