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
        blend: Blend::Alpha,
        distortion: None,
    }
}

#[test]
fn a_decal_lies_in_the_surfaces_plane() {
    let mut m = DecalManager::default();
    for (normal, angle) in [
        ([0.0, -1.0, 0.0], 0.0),
        ([0.0, 0.0, 1.0], 0.7),
        ([0.6, 0.0, 0.8], 2.0),
    ] {
        m.clear();
        m.add(&hole(), [1.0, 2.0, 3.0], normal, angle);
        let d = m.decals().next().unwrap();
        let n = unit(normal.into()).unwrap();
        for k in 0..3 {
            assert!((d.position[k] - [1.0, 2.0, 3.0][k]).abs() < 1e-5);
        }
        let (hx, hy) = (Vec3::from(d.half_x), Vec3::from(d.half_y));
        assert!(hx.dot(n).abs() < 1e-5 && hy.dot(n).abs() < 1e-5);
        assert!(hx.dot(hy).abs() < 1e-5);
        assert!((hx.length() - 0.1).abs() < 1e-5 && (hy.length() - 0.05).abs() < 1e-5);
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
    assert!((dist - 5.0).abs() < 1e-4);
    assert_eq!(d.blend, Blend::Alpha);
    assert!(d.depth_test && !d.depth_write);
    assert_eq!(d.vertices.len(), 6);
    assert!(d.vertices.iter().all(|v| v.color == [255, 255, 255, 128]));
}

#[test]
fn a_multiplied_decal_draws_as_modulate2x_with_its_fade_as_the_alpha() {
    let mut files = std::collections::HashMap::new();
    files.insert("kit/hole.png".to_string(), png(8, 8, 30));
    let files = Files(files);
    let mut set = TextureSet::new(&files);
    let mut m = DecalManager::new(8, 10.0, 2.0);
    let data = DecalData {
        blend: Blend::Modulate2x,
        distortion: None,
        ..hole()
    };
    m.add(&data, [0.0, 5.0, 1.0], [0.0, -1.0, 0.0], 0.0);
    m.advance(9.0);
    let draws = m.draws([0.0, 0.0, 1.0], &mut set);
    assert_eq!(draws.len(), 1);
    let d = &draws[0].1;
    assert_eq!(d.blend, Blend::Modulate2x);
    assert!(d.depth_test && !d.depth_write && d.fog);
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
            blend: Blend::Alpha,
            distortion: None,
        })
    );
    assert_eq!(projectile_decal(&w, 8, &mut rng), None);
}

/// The faces of the box `lo..hi` as triangles, each wound to face out.
fn box_triangles(lo: [f32; 3], hi: [f32; 3]) -> Vec<Triangle> {
    let c = |i: usize| {
        [
            if i & 1 != 0 { hi[0] } else { lo[0] },
            if i & 2 != 0 { hi[1] } else { lo[1] },
            if i & 4 != 0 { hi[2] } else { lo[2] },
        ]
    };
    // Each face's corners counter-clockwise seen from outside.
    let faces = [
        [0, 4, 6, 2],
        [1, 3, 7, 5],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 2, 3, 1],
        [4, 5, 7, 6],
    ];
    faces
        .iter()
        .flat_map(|f| [[c(f[0]), c(f[1]), c(f[2])], [c(f[0]), c(f[2]), c(f[3])]])
        .collect()
}

fn area(d: &Decal) -> f32 {
    d.triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|p| Vec3::from(p.0));
            0.5 * (b - a).cross(c - a).length()
        })
        .sum()
}

#[test]
fn a_clipped_decal_mid_face_is_the_whole_square_on_the_face() {
    // A wall facing -y at y = 0, two units wide.
    let wall = box_triangles([-1.0, 0.0, 0.0], [1.0, 1.0, 2.0]);
    let mut m = DecalManager::default();
    m.add_clipped(&hole(), [0.2, 0.0, 1.0], [0.0, -1.0, 0.0], 0.4, &wall);
    let d = m.decals().next().unwrap();
    assert!((area(d) - 0.2 * 0.1).abs() < 1e-5, "{}", area(d));
    for t in &d.triangles {
        for (p, uv) in t {
            assert!(p[1].abs() < 1e-6, "on the face: {p:?}");
            assert!(uv.iter().all(|c| (-1e-4..=1.0001).contains(c)), "{uv:?}");
        }
    }
}

#[test]
fn a_clipped_decal_at_an_edge_is_cut_off_there_and_none_lies_off_the_surface() {
    // A cube's face at y = 0 whose top edge is at z = 1; the decal sits on the edge, half of it over the air.
    let cube = box_triangles([-1.0, 0.0, -1.0], [1.0, 2.0, 1.0]);
    let data = DecalData {
        size: [0.2, 0.2],
        texture: Some("kit/hole".to_string()),
        blend: Blend::Alpha,
        distortion: None,
    };
    let mut m = DecalManager::default();
    m.add_clipped(&data, [0.0, 0.0, 1.0], [0.0, -1.0, 0.0], 0.3, &cube);
    let d = m.decals().next().unwrap();
    for t in &d.triangles {
        for (p, _) in t {
            assert!(
                p[1].abs() < 1e-6 && p[2] <= 1.0 + 1e-6,
                "on the face, under its edge: {p:?}"
            );
        }
    }
    assert!(
        (area(d) - 0.02).abs() < 1e-4,
        "half the square: {}",
        area(d)
    );
    // Nothing under the box: no decal.
    let mut m = DecalManager::default();
    m.add_clipped(&data, [0.0, -3.0, 1.0], [0.0, -1.0, 0.0], 0.0, &cube);
    assert!(m.is_empty());
}

#[test]
fn a_decal_is_drawn_nearer_the_eye_along_its_ray() {
    let mut files = std::collections::HashMap::new();
    files.insert("kit/hole.png".to_string(), png(8, 8, 30));
    let files = Files(files);
    let mut set = TextureSet::new(&files);
    let mut m = DecalManager::default();
    m.add(&hole(), [0.0, 5.0, 1.0], [0.0, -1.0, 0.0], 0.0);
    let eye = [0.5, 0.0, 1.2];
    let draws = m.draws(eye, &mut set);
    let laid: Vec<[f32; 3]> = m
        .decals()
        .next()
        .unwrap()
        .triangles
        .iter()
        .flat_map(|t| t.map(|p| p.0))
        .collect();
    for (v, p) in draws[0].1.vertices.iter().zip(laid) {
        let to_eye = Vec3::from(eye) - Vec3::from(p);
        let moved = Vec3::from(v.position) - Vec3::from(p);
        assert!(to_eye.cross(moved).length() < 1e-5, "along the eye's ray");
        assert!(moved.dot(to_eye) > 0.0, "toward the eye");
        assert!(v.position[1] < 5.0 && v.position[1] > 4.98);
    }
}
