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

/// The projected path keeps its own cap (held to the hard one) and lays boxes with no mesh, which the CPU path draws nothing
/// of; going back to the CPU path drops them and holds the rest to its cap.
#[test]
fn the_projected_path_keeps_boxes_to_its_cap_and_the_cpu_path_drops_them() {
    let mut m = DecalManager::new(4, 10.0, 2.0);
    assert_eq!(m.projected(), None);
    m.set_projected(Some(usize::MAX));
    assert_eq!(m.projected(), Some(MAX_PROJECTED_DECALS));
    m.set_projected(Some(10));
    for i in 0..12 {
        m.add_box(&hole(), [i as f32, 0.0, 0.0], [0.0, 0.0, 1.0], 0.0);
    }
    assert_eq!(m.len(), 10, "the projected cap, not the CPU path's");
    assert_eq!(m.decals().next().map(|d| d.position[0]), Some(2.0));
    assert!(m.decals().all(|d| d.triangles.is_empty()));
    let files = Files(std::collections::HashMap::new());
    let mut set = TextureSet::new(&files);
    assert!(m.draws([0.0; 3], &mut set).is_empty(), "no mesh to draw");
    m.add(&hole(), [20.0, 0.0, 0.0], [0.0, 0.0, 1.0], 0.0);
    m.set_projected(None);
    assert_eq!(m.len(), 1, "the boxes go; the square stays");
    // A box of no size, or no normal, is not laid.
    m.set_projected(Some(10));
    m.clear();
    m.add_box(
        &DecalData {
            size: [0.0, 1.0],
            ..hole()
        },
        [0.0; 3],
        [0.0, 0.0, 1.0],
        0.0,
    );
    m.add_box(&hole(), [0.0; 3], [0.0; 3], 0.0);
    assert!(m.is_empty());
    m.set_projected(Some(0));
    m.add_box(&hole(), [0.0; 3], [0.0, 0.0, 1.0], 0.0);
    assert!(m.is_empty(), "a cap of none keeps none");
}

/// A decal's box is its square with the CPU path's clip depth through it, its texture the set's, its blend alpha or
/// multiplied, its opacity the vertex alpha's 255ths; one with no texture, faded out or drawn as the bend alone is left out.
#[test]
fn a_decal_projects_as_its_square_with_the_clip_depth_and_the_cpu_paths_alpha() {
    let mut files = std::collections::HashMap::new();
    files.insert("kit/hole.png".to_string(), png(8, 8, 30));
    let files = Files(files);
    let mut set = TextureSet::new(&files);
    let mut m = DecalManager::new(8, 10.0, 2.0);
    m.set_projected(Some(8));
    m.add_box(&hole(), [0.0, 5.0, 1.0], [0.0, -1.0, 0.0], 0.3);
    m.add_box(
        &DecalData {
            blend: Blend::Modulate2x,
            ..hole()
        },
        [1.0, 5.0, 1.0],
        [0.0, -1.0, 0.0],
        0.0,
    );
    m.add_box(
        &DecalData {
            texture: Some("kit/missing".to_string()),
            ..hole()
        },
        [2.0, 5.0, 1.0],
        [0.0, -1.0, 0.0],
        0.0,
    );
    m.advance(9.0);
    let boxes = m.boxes(&mut set);
    assert_eq!(boxes.len(), 2, "the ones whose texture loads");
    let first = m.decals().next().expect("a decal");
    let b = &boxes[0];
    assert_eq!(b.center, first.position);
    assert_eq!((b.half_x, b.half_y), (first.half_x, first.half_y));
    let z = Vec3::from(b.half_z);
    assert!((z.length() - 0.1).abs() < 1e-6, "the longer half side");
    assert!(z.normalize().dot(Vec3::from(first.normal)) > 0.9999);
    assert_eq!(b.blend, Blend::Alpha);
    assert_eq!(boxes[1].blend, Blend::Modulate2x);
    assert_eq!(b.opacity, 128.0 / 255.0);
    assert_eq!(
        b.texture.0, boxes[1].texture.0,
        "one texture, looked up once"
    );
    m.advance(0.999);
    assert!(
        m.boxes(&mut set).is_empty(),
        "faded out (under half a 255th)"
    );
}

/// On the projected path only the bending decals keep a mesh, at most `MAX_PROJECTED_MESHES` of them: past it the oldest
/// mesh is dropped and its decal stays as a box.
#[test]
fn the_projected_path_keeps_at_most_its_cap_of_meshes() {
    let mut m = DecalManager::new(8, 10.0, 2.0);
    m.set_projected(Some(MAX_PROJECTED_MESHES * 4));
    let floor = box_triangles([-500.0, -500.0, -1.0], [500.0, 500.0, 0.0]);
    let extra = 10;
    for i in 0..MAX_PROJECTED_MESHES + extra {
        let at = [i as f32 * 0.5 - 200.0, 0.0, 0.0];
        m.add_clipped(&hole(), at, [0.0, 0.0, 1.0], 0.0, &floor);
    }
    assert_eq!(m.len(), MAX_PROJECTED_MESHES + extra);
    let meshes: Vec<bool> = m.decals().map(|d| !d.triangles.is_empty()).collect();
    assert_eq!(meshes.iter().filter(|&&x| x).count(), MAX_PROJECTED_MESHES);
    assert!(
        meshes[..extra].iter().all(|&x| !x),
        "the oldest lost theirs"
    );
}
