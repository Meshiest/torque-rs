use std::collections::HashSet;
use std::sync::Arc;

use super::*;

/// The legacy importer reading the border word, with TGE's special materials.
static TEST_IMPORTER: maps::legacy::Legacy = maps::legacy::Legacy {
    border_pad: true,
    special: maps::legacy::is_special,
};

/// A profile with fallback folders, as a game's maps may want.
const PROFILE: MapProfile = MapProfile {
    importer: &TEST_IMPORTER,
    interior_fallback: "demo/data/art",
    water_fallback: "packs/water",
    water_texture: "packs/water/water1",
    water_loops_per_unit: 0.25,
    terrain_at_mission_height: true,
};

struct Files(Vec<(String, Vec<u8>)>);

impl Assets for Files {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0
            .iter()
            .find(|(p, _)| p.eq_ignore_ascii_case(path))
            .map(|(_, b)| b.clone())
    }
}

fn object(
    class: MapClass,
    position: [f32; 3],
    scale: [f32; 3],
    fields: &[(&'static str, MapValue)],
) -> Object {
    Object::new(class, position, scale, fields.iter().cloned())
}

fn text(s: &str) -> MapValue {
    MapValue::Text(s.as_bytes().to_vec())
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// A triangle's face normal along `n` (negative: clockwise seen from `n`'s side).
fn facing(t: &[Vertex], n: [f32; 3]) -> f32 {
    let (a, b) = (
        sub(t[1].position, t[0].position),
        sub(t[2].position, t[0].position),
    );
    let c = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    c[0] * n[0] + c[1] * n[1] + c[2] * n[2]
}

/// The interiors' meshes GL-lit (before any relight).
fn interior_meshes(
    objects: &[Object],
    files: &Files,
    textures: &mut TextureSet<'_>,
    notes: &mut Vec<String>,
) -> Vec<Mesh> {
    let instances = interior::instances(objects, &PROFILE, files, notes);
    let materials = interior::materials(&instances, &PROFILE, textures);
    interior::meshes(&instances, &materials, 1, None, notes)
}

/// The synthetic box interior, its one material named `textures/wall`.
fn wall_box_dif() -> Vec<u8> {
    maps::synth::dif(maps::synth::DifSpec {
        version: 0,
        plain_surfaces: true,
        everything: false,
        border: 0,
    })
}

#[test]
fn an_interiors_strips_are_placed_and_wound_clockwise_from_their_fronts() {
    let bytes = wall_box_dif();
    let files = Files(vec![("packs/valley/box.dif".into(), bytes)]);
    let o = object(
        MapClass::InteriorInstance,
        [100.0, 0.0, 5.0],
        [2.0, 2.0, 2.0],
        &[("interiorFile", text("packs/valley/box.dif"))],
    );
    let mut textures = TextureSet::new(&files);
    let mut notes = Vec::new();
    let meshes = interior_meshes(std::slice::from_ref(&o), &files, &mut textures, &mut notes);
    assert_eq!(meshes.len(), 1, "one material: {notes:?}");
    let m = &meshes[0];
    assert!(matches!(&m.texture, TexRef::Missing(n) if n.ends_with("/wall")));
    assert_eq!(m.vertices.len(), 3 * 2 * 6, "six quads");
    for t in m.vertices.chunks_exact(3) {
        assert!(facing(t, t[0].normal) <= 0.0);
        assert!(t.iter().all(|v| v.position[0] >= 50.0));
    }
    let b = interior::bounds(&[o], &PROFILE, &files);
    assert_eq!(b.len(), 1);
    assert!(b[0].0[0] >= 50.0 && b[0].1[0] > b[0].0[0]);
}

#[test]
fn an_interior_material_found_nowhere_beside_its_dif_takes_the_fallback_folders() {
    let bytes = wall_box_dif();
    let files = Files(vec![
        ("packs/valley/box.dif".into(), bytes),
        (
            "demo/data/art/wall.png".into(),
            crate::textures::tests::png(4, 4, 200),
        ),
    ]);
    let o = object(
        MapClass::InteriorInstance,
        [0.0; 3],
        [1.0; 3],
        &[("interiorFile", text("packs/valley/box.dif"))],
    );
    let mut textures = TextureSet::new(&files);
    let meshes = interior_meshes(&[o], &files, &mut textures, &mut Vec::new());
    assert_eq!(meshes[0].texture, TexRef::Name("demo/data/art/wall".into()));
}

#[test]
fn terrain_layers_cover_their_weights_and_the_height_agrees_with_the_mesh() {
    let bytes = maps::synth::ter(3, &["packs/valley/grass", "packs/elsewhere/rock"]);
    // Both textures are found: the first as named, the second beside the terrain.
    let png = crate::textures::tests::png(4, 4, 200);
    let files = Files(vec![
        ("packs/valley/t.ter".into(), bytes),
        ("packs/valley/grass.png".into(), png.clone()),
        ("packs/valley/rock.png".into(), png),
    ]);
    let o = object(
        MapClass::TerrainBlock,
        [0.0, 0.0, 10.0],
        [1.0; 3],
        &[
            ("terrainFile", text("packs/valley/t.ter")),
            ("squareSize", MapValue::Int(8)),
            ("emptySquares", MapValue::Ints(vec![1 << 16])),
        ],
    );
    let mut textures = TextureSet::new(&files);
    let mut block = terrain::Block::load(&o, &PROFILE, &files, &mut textures).unwrap();
    let chunks = terrain::visible_chunks(
        std::slice::from_mut(&mut block),
        [-1000.0, -1000.0, 0.0],
        50.0,
        7,
    );
    assert_eq!(chunks.len(), 1, "one chunk within 50 m of the corner");
    let layers = &chunks[0];
    assert_eq!(layers.len(), 2);
    // The first layer is opaque over every square but the hole at (0, 0).
    assert_eq!(layers[0].vertices.len(), (32 * 32 - 1) * 6);
    assert_eq!(layers[0].blend, Blend::Opaque);
    assert_eq!(layers[1].blend, Blend::Alpha);
    assert_eq!(layers[1].texture, TexRef::Name("packs/valley/rock".into()));
    let ter = mapcore::Importer::heightfield(
        PROFILE.importer,
        &files.read("packs/valley/t.ter").unwrap(),
    )
    .unwrap();
    for t in layers[0].vertices.chunks_exact(3) {
        assert!(facing(t, [0.0, 0.0, 1.0]) < 0.0, "clockwise from above");
        // The triangle's middle lies on the surface the height query answers.
        let mid: [f32; 3] =
            std::array::from_fn(|c| (t[0].position[c] + t[1].position[c] + t[2].position[c]) / 3.0);
        let h = terrain::height_at(&ter, &o, &PROFILE, mid[0], mid[1]).unwrap();
        assert!((h - mid[2]).abs() < 1e-3, "{h} {mid:?}");
    }
    // The second layer's alpha is its share of the two weights.
    for v in layers[1].vertices.iter().take(60) {
        let gx = ((v.position[0] + 1024.0) / 8.0).round() as usize;
        let gy = ((v.position[1] + 1024.0) / 8.0).round() as usize;
        let i = gx + gy * 256;
        let (w0, w1) = (f32::from(ter.weight(0, i)), f32::from(ter.weight(1, i)));
        let want = if w0 + w1 > 0.0 {
            w1 / (w0 + w1) * 255.0
        } else {
            0.0
        };
        assert!((f32::from(v.color[3]) - want).abs() <= 1.0);
    }
    assert!(
        terrain::height_at(&ter, &o, &PROFILE, -1020.0, -1020.0).is_none(),
        "the hole"
    );
}

#[test]
fn a_terrain_material_found_nowhere_is_left_out() {
    let bytes = maps::synth::ter(3, &["packs/valley/grass", "packs/elsewhere/rock"]);
    let files = Files(vec![
        ("packs/valley/t.ter".into(), bytes),
        (
            "packs/valley/grass.png".into(),
            crate::textures::tests::png(4, 4, 200),
        ),
    ]);
    let o = object(
        MapClass::TerrainBlock,
        [0.0; 3],
        [1.0; 3],
        &[
            ("terrainFile", text("packs/valley/t.ter")),
            ("squareSize", MapValue::Int(8)),
        ],
    );
    let mut textures = TextureSet::new(&files);
    let mut block = terrain::Block::load(&o, &PROFILE, &files, &mut textures).unwrap();
    let chunks = terrain::visible_chunks(std::slice::from_mut(&mut block), [0.0; 3], 10.0, 1);
    assert!(!chunks.is_empty());
    assert!(
        chunks
            .iter()
            .all(|c| c.len() == 1 && c[0].blend == Blend::Opaque)
    );
}

#[test]
fn a_water_block_draws_the_top_of_its_box() {
    let files = Files(vec![(
        "packs/water/water1.png".into(),
        crate::textures::tests::png(4, 4, 200),
    )]);
    let o = object(
        MapClass::WaterBlock,
        [10.0, 20.0, 30.0],
        [64.0, 100.0, 5.0],
        &[
            ("surfaceOpacity", MapValue::F32(0.5)),
            ("surfaceTexture", text("data/water/water1")),
        ],
    );
    let mut textures = TextureSet::new(&files);
    let w = water::meshes(&[o], &PROFILE, &mut textures, 1);
    assert_eq!(w.len(), 1);
    assert!(
        w[0].vertices
            .iter()
            .all(|v| v.position[2] == 35.0 && v.color[3] == 128)
    );
    let xs = w[0].vertices.iter().map(|v| v.position[0]);
    let (lo, hi) = xs.fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)));
    assert_eq!((lo, hi), (10.0, 74.0));
    // Found in the profile's water folder by its file name.
    assert_eq!(w[0].texture, TexRef::Name("packs/water/water1".into()));
}

#[test]
fn a_kept_mesh_is_sent_once_and_named_by_its_key_after() {
    let files = Files(Vec::new());
    let mut textures = TextureSet::new(&files);
    let vertex = Vertex {
        position: [0.0; 3],
        normal: [0.0, 0.0, 1.0],
        color: [255; 4],
        uv: [0.0; 2],
    };
    let mesh = Mesh {
        label: "x".into(),
        texture: TexRef::None,
        vertices: Arc::new(vec![vertex; 3]),
        lighting: true,
        tex_env: TexEnv::None,
        blend: Blend::Opaque,
        cull: Cull::None,
        depth_write: true,
        sampler: SAMPLER,
        alpha_from_texture: false,
        overlay: false,
        centre: [0.0; 3],
        key: 42,
    };
    let mut sent = MapSent::default();
    for first in [true, false] {
        let mut out = MapDraws::default();
        let mut keys = HashSet::new();
        emit(
            &mesh,
            &mut textures,
            Some(&sent.keys),
            &mut keys,
            [0.0; 3],
            &mut out,
        );
        sent.keys = keys;
        assert_eq!(out.opaque[0].vertex_key, Some(42));
        assert_eq!(out.opaque[0].vertices.len(), if first { 3 } else { 0 });
    }
    let mut out = MapDraws::default();
    emit(
        &mesh,
        &mut textures,
        None,
        &mut HashSet::new(),
        [0.0; 3],
        &mut out,
    );
    assert_eq!(
        (out.opaque[0].vertex_key, out.opaque[0].vertices.len()),
        (None, 3)
    );
}

#[test]
fn the_identity_follows_every_field() {
    let a = MapRecords {
        objects: vec![object(
            MapClass::WaterBlock,
            [1.0; 3],
            [1.0; 3],
            &[("surfaceOpacity", MapValue::F32(1.0))],
        )],
        statics: Vec::new(),
    };
    let mut b = MapRecords {
        objects: a.objects.clone(),
        statics: Vec::new(),
    };
    assert_eq!(identity(&a), identity(&b));
    b.objects[0].fields[0].values[0] = MapValue::F32(2.0);
    assert_ne!(identity(&a), identity(&b));
}

#[test]
fn shape_replicas_land_on_the_terrain_the_same_way_each_time() {
    let bytes = maps::synth::ter(3, &["packs/valley/grass"]);
    let files = Files(vec![
        ("packs/valley/t.ter".into(), bytes),
        (
            "packs/valley/grass.png".into(),
            crate::textures::tests::png(4, 4, 200),
        ),
    ]);
    let t = object(
        MapClass::TerrainBlock,
        [0.0; 3],
        [1.0; 3],
        &[
            ("terrainFile", text("packs/valley/t.ter")),
            ("squareSize", MapValue::Int(8)),
            ("RepeatTerrain", MapValue::Bool(true)),
        ],
    );
    let r = object(
        MapClass::FxShapeReplicator,
        [5.0, 5.0, 0.0],
        [1.0; 3],
        &[
            ("Seed", MapValue::Int(1376312589)),
            ("ShapeFile", text("packs/valley/tree.dts")),
            ("ShapeCount", MapValue::Int(50)),
            ("ShapeRetries", MapValue::Int(10)),
            ("OuterRadiusX", MapValue::Int(100)),
            ("OuterRadiusY", MapValue::Int(60)),
            ("AllowOnTerrain", MapValue::Bool(true)),
            ("AllowedTerrainSlope", MapValue::Int(90)),
            ("ShapeScaleMin", MapValue::Point([1.0, 1.0, 1.0])),
            ("ShapeScaleMax", MapValue::Point([2.0, 2.0, 2.0])),
        ],
    );
    let mut textures = TextureSet::new(&files);
    let block = terrain::Block::load(&t, &PROFILE, &files, &mut textures).unwrap();
    let blocks = [block];
    let a = foliage::shape_replicas(std::slice::from_ref(&r), &blocks, &[]);
    let b = foliage::shape_replicas(std::slice::from_ref(&r), &blocks, &[]);
    assert_eq!(a.len(), 50);
    assert_eq!(
        a.iter().map(|x| x.1.m).collect::<Vec<_>>(),
        b.iter().map(|x| x.1.m).collect::<Vec<_>>()
    );
    for (_, m, s) in &a {
        let (x, y, z) = (m.m[3], m.m[7], m.m[11]);
        assert!((x - 5.0).abs() <= 100.0 && (y - 5.0).abs() <= 60.0);
        let ground = blocks[0].surface(x, y).unwrap().0;
        assert!((ground - z).abs() < 1e-3);
        assert!(s.iter().all(|c| (1.0..=2.0).contains(c)));
    }
}

/// A map asking for more replicas than the engine draws, over many replicators: all of them together place at most
/// [`foliage::MAX_REPLICAS_TOTAL`] (the work and memory are the engine's to bound, not the map's).
#[test]
fn many_replicators_share_one_replica_budget() {
    let bytes = maps::synth::ter(3, &["packs/valley/grass"]);
    let files = Files(vec![
        ("packs/valley/t.ter".into(), bytes),
        (
            "packs/valley/grass.png".into(),
            crate::textures::tests::png(4, 4, 200),
        ),
    ]);
    let t = object(
        MapClass::TerrainBlock,
        [0.0; 3],
        [1.0; 3],
        &[
            ("terrainFile", text("packs/valley/t.ter")),
            ("squareSize", MapValue::Int(8)),
            ("RepeatTerrain", MapValue::Bool(true)),
        ],
    );
    let r = object(
        MapClass::FxShapeReplicator,
        [5.0, 5.0, 0.0],
        [1.0; 3],
        &[
            ("ShapeFile", text("packs/valley/tree.dts")),
            ("ShapeCount", MapValue::Int(1_000_000)),
            ("ShapeRetries", MapValue::Int(1)),
            ("OuterRadiusX", MapValue::Int(100)),
            ("OuterRadiusY", MapValue::Int(60)),
            ("AllowOnTerrain", MapValue::Bool(true)),
            ("AllowedTerrainSlope", MapValue::Int(90)),
            ("ShapeScaleMin", MapValue::Point([1.0, 1.0, 1.0])),
            ("ShapeScaleMax", MapValue::Point([1.0, 1.0, 1.0])),
        ],
    );
    let mut textures = TextureSet::new(&files);
    let block = terrain::Block::load(&t, &PROFILE, &files, &mut textures).unwrap();
    let blocks = [block];
    let many = vec![r.clone(); 8];
    let placed = foliage::shape_replicas(&many, &blocks, &[]);
    assert!(
        placed.len() <= foliage::MAX_REPLICAS_TOTAL,
        "{}",
        placed.len()
    );
    assert!(placed.len() > foliage::MAX_REPLICAS, "{}", placed.len());
    // Replicators whose every try misses (their ring off a block that does not repeat) cost their tries all the same: many of
    // them stop at the total of tries, not at a replica count none of them reaches.
    let mut off = r;
    off.position = [1.0e6, 1.0e6, 0.0];
    let lone = object(
        MapClass::TerrainBlock,
        [0.0; 3],
        [1.0; 3],
        &[
            ("terrainFile", text("packs/valley/t.ter")),
            ("squareSize", MapValue::Int(8)),
        ],
    );
    let blocks = [terrain::Block::load(&lone, &PROFILE, &files, &mut textures).unwrap()];
    let start = std::time::Instant::now();
    let none = foliage::shape_replicas(&vec![off; 1_000], &blocks, &[]);
    assert!(none.is_empty());
    assert!(start.elapsed().as_secs() < 20, "{:?}", start.elapsed());
}

// ---- the sun's relight

/// A flat terrain at height 0 (one metre squares; the block spans -128..128).
fn flat_terrain(repeat: bool) -> light::TerrainIn {
    let ter = mapcore::collision::Heightfield {
        heights: vec![0.0; maps::ter::SAMPLES],
        layers: Vec::new(),
        split: mapcore::collision::Split::Alternating,
    };
    light::TerrainIn {
        ter: Arc::new(ter),
        origin: [-128.0, -128.0, 0.0],
        square: 1.0,
        repeat,
        empty: Arc::new(vec![false; maps::ter::SAMPLES]),
        hash: 7,
    }
}

/// A box's twelve triangles (material "m", no lightmap rectangles).
fn box_local(min: [f32; 3], max: [f32; 3]) -> interior::Local {
    let c = |k: usize| {
        [
            if k & 1 == 0 { min[0] } else { max[0] },
            if k & 2 == 0 { min[1] } else { max[1] },
            if k & 4 == 0 { min[2] } else { max[2] },
        ]
    };
    let faces = [
        [0, 1, 3, 2],
        [4, 6, 7, 5],
        [0, 4, 5, 1],
        [2, 3, 7, 6],
        [0, 2, 6, 4],
        [1, 5, 7, 3],
    ];
    let mut tris = Vec::new();
    for f in faces {
        for t in [[f[0], f[1], f[2]], [f[0], f[2], f[3]]] {
            for k in t {
                tris.push((c(k), [0.0, 0.0, 1.0], [0.0, 0.0]));
            }
        }
    }
    let n = tris.len() / 3;
    let mut l = interior::Local::default();
    l.by_material.insert("m".into(), tris);
    let none = mapcore::geometry::LightmapTri {
        rect: mapcore::geometry::NO_RECT,
        texel: [[0.0; 2]; 3],
    };
    l.lightmap_tris.insert("m".into(), vec![none; n]);
    l.bounds = (min, max);
    l.content = 11;
    l
}

/// A floor quad at z = 0 from (0, 0) to (8, 8) with an 8 x 8 lightmap rectangle (one texel a unit) in an 8 x 8 lightmap.
fn floor_local() -> interior::Local {
    let pts = vec![
        [0.0, 0.0, 0.0],
        [8.0, 0.0, 0.0],
        [0.0, 8.0, 0.0],
        [8.0, 8.0, 0.0],
    ];
    let mut l = interior::Local::default();
    let up = [0.0, 0.0, 1.0];
    let tris = [0, 1, 2, 1, 3, 2]
        .iter()
        .map(|&k| (pts[k], up, [0.0; 2]))
        .collect();
    let texel = |p: [f32; 3]| [p[0], p[1]];
    l.by_material.insert("floor".into(), tris);
    l.lightmap_tris.insert(
        "floor".into(),
        vec![
            mapcore::geometry::LightmapTri {
                rect: 0,
                texel: [texel(pts[0]), texel(pts[1]), texel(pts[2])],
            },
            mapcore::geometry::LightmapTri {
                rect: 0,
                texel: [texel(pts[1]), texel(pts[3]), texel(pts[2])],
            },
        ],
    );
    l.rects.push(mapcore::geometry::LightRect {
        lightmap: 0,
        offset: [0, 0],
        size: [8, 8],
        axes: [0, 1],
        scale: [1.0, 1.0],
        add: [0.0, 0.0],
        normal: up,
        d: 0.0,
        polys: vec![pts],
    });
    l.lightmaps.push(mapcore::material::EncodedImage {
        encoding: mapcore::material::Encoding::Png,
        width: 8,
        height: 8,
        bytes: Vec::new(),
    });
    l.bounds = ([0.0; 3], [8.0, 8.0, 0.0]);
    l.content = 13;
    l
}

fn instance(local: interior::Local, at: [f32; 3]) -> light::InstanceIn {
    let mut m = crate::math::Mat::IDENTITY;
    m.set_column3(at);
    light::InstanceIn {
        local: Arc::new(local),
        matrix: m,
        scale: [1.0; 3],
        opaque: vec![true],
    }
}

fn overhead_sun() -> Sun {
    Sun {
        towards: [0.0, 0.0, 1.0],
        diffuse: [0.5; 3],
        ambient: [0.3; 3],
    }
}

fn texel(t: &Texture, x: usize, y: usize) -> u8 {
    t.levels[0][(y * t.width as usize + x) * 4]
}

fn level(v: f32) -> u8 {
    (v * 255.0).round() as u8
}

#[test]
fn the_sun_shadows_the_terrain_under_a_roof_and_lights_open_ground() {
    let inputs = light::Inputs {
        sun: overhead_sun(),
        terrains: vec![flat_terrain(false)],
        instances: vec![instance(
            box_local([-10.0, -10.0, 0.5], [10.0, 10.0, 8.0]),
            [0.0; 3],
        )],
    };
    let key = inputs.key();
    let (lit, report) = light::make(&inputs, key, light::plan(&inputs));
    assert_eq!(report.casters, 12);
    assert_eq!(report.over_budget, 0);
    let t = &lit.terrains[0];
    // Two texels a metre: (0, 0) is texel 256; (50, 50) is texel 356.
    assert_eq!(texel(t, 256, 256), level(0.3));
    assert_eq!(texel(t, 356, 356), level(0.8));
    let r = light::terrain_reading(&lit, &inputs, 0.0, 0.0).unwrap();
    assert!((r[1] - r[0]) / (r[1] - r[2]) > 0.99, "{r:?}");
    assert_eq!(light::roofs(&lit, &inputs, 20).len(), 1);
}

#[test]
fn a_repeated_terrain_takes_the_interiors_shadows_only_on_their_own_tile() {
    // The roof stands in tile (1, 0) of a block repeating every 256 m.
    let inputs = light::Inputs {
        sun: overhead_sun(),
        terrains: vec![flat_terrain(true)],
        instances: vec![instance(
            box_local([-10.0, -10.0, 2.0], [10.0, 10.0, 8.0]),
            [256.0, 0.0, 0.0],
        )],
    };
    let plan = light::plan(&inputs);
    // The tiles within reach of the roof's bounds (at most MAX_TILES), then the terrain-only lightmap.
    assert_eq!(plan.terrains.len(), light::MAX_TILES + 1);
    let own = plan.terrain_map(0, [1, 0]).unwrap();
    let rest = plan.terrain_map(0, [5, 5]).unwrap();
    assert!(plan.terrains[own].interiors && !plan.terrains[rest].interiors);
    let (lit, _) = light::make(&inputs, inputs.key(), plan);
    assert_eq!(texel(&lit.terrains[own], 256, 256), level(0.3));
    assert_eq!(texel(&lit.terrains[rest], 256, 256), level(0.8));
    let r = light::terrain_reading(&lit, &inputs, 256.0, 0.0).unwrap();
    assert!(r[0] < r[1]);
    let r = light::terrain_reading(&lit, &inputs, 0.0, 0.0).unwrap();
    assert_eq!(r[0], r[1]);
}

#[test]
fn the_tiles_with_buildings_get_their_own_lightmaps_before_tiles_a_wide_plane_only_crosses() {
    // A deck 5 km wide high over the terrain (as NYC's city plane) lies first in tile order; a roof stands in tile (6, 5).
    let inputs = light::Inputs {
        sun: overhead_sun(),
        terrains: vec![flat_terrain(true)],
        instances: vec![
            instance(
                box_local([-2500.0, -2500.0, 1900.0], [2500.0, 2500.0, 1901.0]),
                [-3000.0, -3000.0, 0.0],
            ),
            instance(
                box_local([-10.0, -10.0, 0.5], [10.0, 10.0, 8.0]),
                [1536.0, 1280.0, 0.0],
            ),
        ],
    };
    let plan = light::plan(&inputs);
    let roof = [6, 5];
    let own = plan.terrain_map(0, roof).unwrap();
    assert!(plan.terrains[own].interiors && plan.terrains[own].tile == roof);
    let (lit, _) = light::make(&inputs, inputs.key(), plan);
    let r = light::terrain_reading(&lit, &inputs, 1536.0, 1280.0).unwrap();
    assert!((r[1] - r[0]) / (r[1] - r[2]) > 0.99, "{r:?}");
    assert_eq!(light::roofs(&lit, &inputs, 20).len(), 1);
}

#[test]
fn an_interiors_rectangle_is_lit_in_its_atlas_place_with_a_gutter_and_its_pass_samples_it() {
    // The floor at the origin, a slab over its half x < 4.
    let inputs = light::Inputs {
        sun: overhead_sun(),
        terrains: Vec::new(),
        instances: vec![
            instance(floor_local(), [0.0; 3]),
            instance(box_local([-1.0, -1.0, 3.0], [4.0, 9.0, 4.0]), [0.0; 3]),
        ],
    };
    let plan = light::plan(&inputs);
    assert_eq!(plan.pages.len(), 1);
    let p = plan.places[0][0].unwrap();
    let (lit, _) = light::make(&inputs, inputs.key(), plan);
    let page = &lit.pages[0];
    let (x0, y0) = (p.x as usize + 1, p.y as usize + 1);
    for j in 0..8 {
        for i in 0..8 {
            let want = if i < 4 { level(0.3) } else { level(0.8) };
            assert_eq!(texel(page, x0 + i, y0 + j), want, "texel {i} {j}");
        }
    }
    // The gutter repeats the edge.
    assert_eq!(texel(page, x0 - 1, y0 + 2), texel(page, x0, y0 + 2));
    assert_eq!(texel(page, x0 + 8, y0 + 2), texel(page, x0 + 7, y0 + 2));
    // The floor draws unlit, then multiplied by its page.
    let instances = vec![interior::Instance {
        folder: "packs/valley".into(),
        local: Arc::clone(&inputs.instances[0].local),
        matrix: crate::math::Mat::IDENTITY,
        scale: [1.0; 3],
    }];
    let materials = vec![vec![interior::Material {
        group: "floor".into(),
        texture: TexRef::None,
        opaque: true,
    }]];
    let meshes = interior::meshes(&instances, &materials, 1, Some(&lit), &mut Vec::new());
    assert_eq!(meshes.len(), 2);
    assert!(!meshes[0].lighting);
    let pass = &meshes[1];
    assert_eq!(pass.blend, Blend::Multiply);
    assert!(pass.overlay && !pass.depth_write);
    assert!(matches!(&pass.texture, TexRef::Generated(t) if Arc::ptr_eq(t, page)));
    for v in pass.vertices.iter() {
        let u = v.uv[0] * page.width as f32 - x0 as f32;
        let w = v.uv[1] * page.height as f32 - y0 as f32;
        assert!(
            (0.0..=8.0).contains(&u) && (0.0..=8.0).contains(&w),
            "{u} {w}"
        );
    }
    let positions = |m: &Mesh| m.vertices.iter().map(|v| v.position).collect::<Vec<_>>();
    assert_eq!(positions(pass), positions(&meshes[0]));
    // Not opaque: GL-lit, no pass.
    let mut glass = materials.clone();
    glass[0][0].opaque = false;
    let meshes = interior::meshes(&instances, &glass, 1, Some(&lit), &mut Vec::new());
    assert!(meshes[0].lighting);
    assert!(meshes[1].vertices.is_empty());
}

#[test]
fn a_texel_outside_its_polygon_is_moved_onto_it() {
    let l = floor_local();
    let r = &l.rects[0];
    let p = light::texel_point(r, -3.0, 4.0, &mut u64::MAX.clone()).unwrap();
    assert!(
        p[0] >= 0.0 && p[0] < 0.1 && (p[1] - 4.0).abs() < 0.1 && p[2] == 0.0,
        "{p:?}"
    );
    assert_eq!(
        light::texel_point(r, 2.5, 6.5, &mut u64::MAX.clone()),
        Some([2.5, 6.5, 0.0])
    );
    // The triangles tried are charged to the job's work; with none left the point stays on the plane, unclamped.
    let mut work = 1_000;
    light::texel_point(r, -3.0, 4.0, &mut work);
    assert!(work < 1_000);
    let mut none = 0;
    assert_eq!(
        light::texel_point(r, -3.0, 4.0, &mut none),
        Some([-3.0, 4.0, 0.0])
    );
}

#[test]
fn the_same_inputs_light_the_same_and_the_cache_file_reads_back_only_for_its_plan() {
    let inputs = light::Inputs {
        sun: Sun {
            towards: [0.3, 0.2, 0.9],
            diffuse: [0.6, 0.5, 0.4],
            ambient: [0.2; 3],
        },
        terrains: vec![flat_terrain(true)],
        instances: vec![
            instance(floor_local(), [5.0, 5.0, 1.0]),
            instance(box_local([0.0, 0.0, 3.0], [6.0, 6.0, 5.0]), [0.0; 3]),
        ],
    };
    let key = inputs.key();
    let (a, _) = light::make(&inputs, key, light::plan(&inputs));
    let (b, _) = light::make(&inputs, key, light::plan(&inputs));
    let bytes = light::encode(&a);
    assert_eq!(bytes, light::encode(&b));
    let back = light::decode(&bytes, key, &a.plan).unwrap();
    assert_eq!(light::encode(&back), bytes);
    assert!(light::decode(&bytes, key ^ 1, &a.plan).is_none());
    assert!(light::decode(&bytes[..bytes.len() - 1], key, &a.plan).is_none());
    let mut bad = bytes.clone();
    bad[0] ^= 1;
    assert!(light::decode(&bad, key, &a.plan).is_none());
    let mut other = a.plan.clone();
    other.terrains.pop();
    assert!(light::decode(&bytes, key, &other).is_none());
    // Another sun is another key.
    let mut night = inputs.clone();
    night.sun.diffuse = [0.1; 3];
    assert_ne!(night.key(), key);
}

#[test]
fn the_atlas_never_passes_its_pages_and_a_ray_stops_when_its_work_is_spent() {
    let mut l = floor_local();
    let side = light::MAX_RECT_SIDE;
    l.rects[0].size = [side, side];
    let inst = instance(l, [0.0; 3]);
    let inputs = light::Inputs {
        sun: overhead_sun(),
        terrains: Vec::new(),
        instances: vec![inst; light::MAX_PAGES + 4],
    };
    let plan = light::plan(&inputs);
    assert_eq!(plan.pages.len(), light::MAX_PAGES);
    let placed = plan.places.iter().flatten().filter(|p| p.is_some()).count();
    assert_eq!(placed, light::MAX_PAGES);
    let bvh = light::Bvh::new(vec![[[-1.0, -1.0, 1.0], [1.0, -1.0, 1.0], [0.0, 1.0, 1.0]]]);
    let mut work = 100;
    assert!(bvh.hits([0.0; 3], [0.0, 0.0, 1.0], 10.0, &mut work));
    let mut none = 0;
    assert!(!bvh.hits([0.0; 3], [0.0, 0.0, 1.0], 10.0, &mut none));
}

#[test]
fn a_difs_lightmap_rectangles_are_read_within_their_lightmap() {
    let bytes = maps::synth::dif(maps::synth::DifSpec {
        version: 0,
        plain_surfaces: true,
        everything: false,
        border: 0,
    });
    let mut d = maps::dif::parse_with_border_pad(&bytes).unwrap();
    // The synthetic file's lightmap is 1 x 1, smaller than its surfaces' 8 x 8 rectangles: none is used.
    let l = maps::legacy::surfaces_with(&d, maps::legacy::is_special);
    assert!(l.rects.is_empty());
    assert!(
        l.lightmap_tris
            .values()
            .flatten()
            .all(|t| t.rect == mapcore::geometry::NO_RECT)
    );
    d.details[0].lightmaps[0].png.width = 8;
    d.details[0].lightmaps[0].png.height = 8;
    let l = maps::legacy::surfaces_with(&d, maps::legacy::is_special);
    // The two faces across z share one rectangle (the same lightmap, offset and size); the others are edge-on to the
    // lightmap's x and y planes and have none.
    assert_eq!(l.rects.len(), 1);
    assert_eq!(l.rects[0].polys.len(), 2);
    assert_eq!(
        l.lightmap_tris
            .values()
            .flatten()
            .filter(|t| t.rect == 0)
            .count(),
        4
    );
    d.details[0].lightmaps[0].png.width = light::MAX_SOURCE_SIDE + 1;
    assert!(
        maps::legacy::surfaces_with(&d, maps::legacy::is_special)
            .rects
            .is_empty()
    );
}

#[test]
fn a_generated_texture_enters_a_frames_set_once() {
    let files = Files(Vec::new());
    let mut textures = TextureSet::new(&files);
    let t = Arc::new(Texture {
        path: "map lightmap 1 0".into(),
        width: 2,
        height: 2,
        levels: Arc::new(vec![vec![255; 16]]),
    });
    let a = textures.generated(&t);
    assert!(a.is_some());
    assert_eq!(textures.generated(&t), a);
    assert_eq!(textures.len(), 1);
}

/// A thousand shape replicators over one interior, each asking for many replicas, share one work budget: once it is spent,
/// the replicators after place nothing, and each box test is charged (the interiors are a server's files).
#[test]
fn many_replicators_share_one_work_budget() {
    let files = Files(vec![("packs/valley/box.dif".into(), wall_box_dif())]);
    let interior = object(
        MapClass::InteriorInstance,
        [0.0; 3],
        [1.0; 3],
        &[("interiorFile", text("packs/valley/box.dif"))],
    );
    let r = object(
        MapClass::FxShapeReplicator,
        [0.0; 3],
        [1.0; 3],
        &[
            ("ShapeFile", text("packs/valley/tree.dts")),
            ("OuterRadiusX", MapValue::Int(2)),
            ("OuterRadiusY", MapValue::Int(2)),
            ("AllowOnInteriors", MapValue::Bool(true)),
        ],
    );
    let ground = foliage::interior_ground(&[interior, r.clone()], &PROFILE, &files);
    assert_eq!(ground.len(), 1);
    let cap = 20_000u64;
    let (mut tries, mut total) = (usize::MAX, 0u64);
    let (mut placed, mut placed_past) = (0usize, 0usize);
    for _ in 0..1_000 {
        let spent = total > cap;
        let p = foliage::places(&r, &[], &ground, 1_000, 4, &mut tries, &mut total, cap);
        placed += p.len();
        if spent {
            placed_past += p.len();
        }
    }
    assert!(
        placed > 0,
        "replicas land on the interior until the work is spent"
    );
    assert!(total > cap, "the shared work ran out: {total}");
    assert_eq!(placed_past, 0, "no replicator places past the shared work");
    // A try with no interior near costs its box test all the same.
    let mut total = 0u64;
    let far = object(
        MapClass::FxShapeReplicator,
        [1.0e5, 1.0e5, 0.0],
        [1.0; 3],
        &[],
    );
    let mut tries = 10;
    let p = foliage::places(&far, &[], &ground, 0, 1, &mut tries, &mut total, cap);
    assert!(p.is_empty());
    let mut far = far;
    far.fields.push(MapField {
        name: "AllowOnInteriors",
        values: vec![MapValue::Bool(true)],
    });
    let p = foliage::places(&far, &[], &ground, 10, 1, &mut tries, &mut total, cap);
    assert!(p.is_empty());
    assert_eq!(total, 10, "a box test a try");
}

/// Hundreds of huge interior instances merge at most the vertex cap, with a note.
#[test]
fn many_huge_interiors_merge_at_most_the_vertex_cap() {
    let files = Files(vec![("packs/valley/box.dif".into(), wall_box_dif())]);
    let objects: Vec<Object> = (0..500)
        .map(|i| {
            object(
                MapClass::InteriorInstance,
                [i as f32 * 20.0, 0.0, 0.0],
                [1.0e5; 3],
                &[("interiorFile", text("packs/valley/box.dif"))],
            )
        })
        .collect();
    let mut textures = TextureSet::new(&files);
    let mut notes = Vec::new();
    let instances = interior::instances(&objects, &PROFILE, &files, &mut notes);
    let materials = interior::materials(&instances, &PROFILE, &mut textures);
    let all: usize = interior::meshes(&instances, &materials, 1, None, &mut notes)
        .iter()
        .map(|m| m.vertices.len())
        .sum();
    assert_eq!(all, 500 * 36, "under the cap every instance merges");
    assert!(notes.iter().all(|n| !n.contains("past")));
    let cap = 36 * 100 + 4;
    let capped: usize = interior::meshes_capped(&instances, &materials, 1, None, &mut notes, cap)
        .iter()
        .map(|m| m.vertices.len())
        .sum();
    assert!(capped >= cap && capped <= cap + 2, "{capped}");
    assert!(notes.iter().any(|n| n.contains("past")), "{notes:?}");
}

/// A shape with many triangles placed by thousands of statics merges at most the vertex cap, with a note.
#[test]
fn many_statics_merge_at_most_the_vertex_cap() {
    struct Big;
    impl RestShapes for Big {
        fn rest_draws(
            &self,
            _: &dyn Assets,
            _: &str,
            _: &mut TextureSet<'_>,
            _: &mut Vec<String>,
        ) -> Option<Vec<Draw>> {
            let v = Vertex {
                position: [0.0; 3],
                normal: [0.0, 0.0, 1.0],
                color: [255; 4],
                uv: [0.0; 2],
            };
            Some(vec![Draw {
                label: "big".into(),
                vertices: vec![v; 3 * 1_000],
                lighting: true,
                tex_env: TexEnv::None,
                texture: None,
                blend: Blend::Opaque,
                cull: Cull::None,
                depth_test: true,
                depth_write: true,
                fog: true,
                lights: Vec::new(),
                vertex_key: None,
            }])
        }
    }
    let files = Files(Vec::new());
    let mut textures = TextureSet::new(&files);
    let mut notes = Vec::new();
    let id = crate::math::Mat::IDENTITY.m;
    let statics: Vec<(String, [f32; 16], [f32; 3])> = (0..2_000)
        .map(|_| ("packs/valley/big.dts".to_string(), id, [1.0; 3]))
        .collect();
    let cap = 3_000 * 10 + 1;
    let meshes = statics::meshes(
        &Big,
        &files,
        &statics,
        &[],
        &mut textures,
        1,
        &mut notes,
        cap,
    );
    let total: usize = meshes.iter().map(|m| m.vertices.len()).sum();
    assert!(total >= cap && total <= cap + 2, "{total}");
    assert!(notes.iter().any(|n| n.contains("past")), "{notes:?}");
}
