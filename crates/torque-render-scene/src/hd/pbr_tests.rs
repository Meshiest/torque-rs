use super::*;

/// An RGBA PNG of `w` x `h` from `rgba` (stored deflate blocks).
fn png_rgba(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    png_of(w, h, 6, rgba)
}

/// A PNG of `w` x `h` of color type `color` (6 RGBA, 0 gray) from its bytes, 8 bits a sample.
fn png_of(w: u32, h: u32, color: u8, pixels: &[u8]) -> Vec<u8> {
    use torque_bytes::crc32;

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc32(&c).to_be_bytes());
    }
    let mut raw = Vec::new();
    let samples = if color == 6 { 4 } else { 1 };
    for row in pixels.chunks(samples * w as usize) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (k, b) in blocks.iter().enumerate() {
        z.push(u8::from(k + 1 == blocks.len()));
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for x in &raw {
        a = (a + u32::from(*x)) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = w.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, color, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn solid(w: u32, h: u32, p: [u8; 4]) -> Vec<u8> {
    (0..w * h).flat_map(|_| p).collect()
}

#[test]
fn color_maps_are_filtered_in_linear_light() {
    // Black and white texels average to half the light: sRGB 188, not 128.
    let rgba = [
        0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255,
    ];
    let levels = color_levels(&rgba, 2, 2, None);
    assert_eq!(levels.len(), 2);
    assert_eq!(&levels[1][..3], &[188, 188, 188]);
    assert_eq!(levels[1][3], 255);
    // Odd and uneven sizes reach 1 x 1 with every level its size.
    let levels = color_levels(&solid(5, 3, [10, 20, 30, 40]), 5, 3, None);
    let sizes: Vec<usize> = levels.iter().map(Vec::len).collect();
    assert_eq!(sizes, vec![60, 8, 4]);
    assert!(levels.iter().all(|l| l[..4] == [10, 20, 30, 40]));
}

#[test]
fn a_bumpy_normal_maps_lower_levels_are_unit_and_widen_the_roughness() {
    // Normals tilted 45 degrees left and right in turn: the level below points straight out, its mean length cos 45.
    let (l, r) = ([38, 128, 218, 255], [217, 128, 218, 255]);
    let rgba: Vec<u8> = [l, r, r, l].concat();
    let (levels, lengths) = normal_levels(&rgba, 2, 2, NormalConvention::OpenGl);
    assert_eq!(levels[1][..3], [128, 128, 255]);
    assert!(
        (lengths[1][0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.02,
        "{}",
        lengths[1][0]
    );
    let orm = orm_levels(
        &solid(2, 2, [255, 64, 0, 255]),
        2,
        2,
        Some((2, 2, &lengths)),
    );
    assert_eq!(orm[0][1], 64, "level 0 keeps its roughness");
    assert!(
        orm[1][1] > 200,
        "the bumps' level below is rough: {}",
        orm[1][1]
    );
    // A flat map leaves the roughness as it was.
    let (_, flat) = normal_levels(
        &solid(2, 2, [128, 128, 255, 255]),
        2,
        2,
        NormalConvention::OpenGl,
    );
    let orm = orm_levels(&solid(2, 2, [255, 64, 0, 255]), 2, 2, Some((2, 2, &flat)));
    assert!(orm[1][1].abs_diff(64) <= 1);
}

#[test]
fn a_directx_normal_map_is_flipped_to_opengls_convention() {
    let up = [128, 230, 200, 255];
    let (gl, _) = normal_levels(&solid(1, 1, up), 1, 1, NormalConvention::OpenGl);
    let (dx, _) = normal_levels(&solid(1, 1, up), 1, 1, NormalConvention::DirectX);
    assert!(gl[0][1] > 200);
    assert!(dx[0][1] < 60);
    assert_eq!(gl[0][2], dx[0][2]);
}

#[test]
fn the_uniform_holds_the_factors_maps_and_alpha_mode() {
    let mut m = HdMaterial::of_factors(&PbrMaterial {
        roughness: 0.25,
        alpha: AlphaMode::Cutout,
        ..PbrMaterial::default()
    });
    m.normal = Some(3);
    m.height = Some(4);
    let b = m.uniform_bytes();
    let f = |i: usize| f32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap());
    let u = |i: usize| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap());
    assert_eq!(f(0), 1.0);
    assert_eq!(f(8), 0.25);
    assert_eq!(u(16), map_bits::NORMAL | map_bits::HEIGHT);
    assert_eq!(u(17), 1);
}

#[test]
fn a_table_keeps_each_distinct_material_once() {
    let mut t = MaterialTable::default();
    let a = HdMaterial::of_factors(&PbrMaterial::default());
    let mut b = a.clone();
    b.metal = 0.0;
    assert_eq!(t.intern(a.clone()), MaterialId(0));
    assert_eq!(t.intern(b), MaterialId(1));
    assert_eq!(t.intern(a), MaterialId(0));
    assert_eq!(t.materials.len(), 2);
}

#[test]
fn a_sidecars_maps_load_into_the_texture_table() {
    let files: HashMap<String, Vec<u8>> = [
        ("c.png", png_rgba(4, 4, &solid(4, 4, [200, 100, 50, 255]))),
        ("n.png", png_rgba(4, 4, &solid(4, 4, [128, 128, 255, 255]))),
        ("r.png", png_rgba(4, 4, &solid(4, 4, [90, 90, 90, 255]))),
        ("m.png", png_rgba(4, 4, &solid(4, 4, [255, 255, 255, 255]))),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let m = PbrMaterial {
        base: Some("c.png".into()),
        normal: Some("n.png".into()),
        roughness_map: Some("r.png".into()),
        metal_map: Some("m.png".into()),
        ..PbrMaterial::default()
    };
    let mut textures = TextureTable::default();
    let mut read = |p: &str| files.get(p).cloned();
    let got = load(&m, "mats/a", &mut read, &mut textures).expect("loads");
    assert_eq!(textures.textures.len(), 3);
    let orm = &textures.textures[got.orm.expect("packed")];
    assert_eq!(&orm.levels[0][..4], &[255, 90, 255, 255]);
    assert_eq!(orm.levels.len(), 3);
    assert!(got.height.is_none() && got.emissive.is_none());
    // A second material of the same files under another sidecar has its own maps.
    let mut read = |p: &str| files.get(p).cloned();
    load(&m, "mats/b", &mut read, &mut textures).expect("loads");
    assert_eq!(textures.textures.len(), 6);
}

#[test]
fn maps_that_are_missing_unknown_huge_or_broken_fail_the_material() {
    let mut huge = png_rgba(1, 1, &[0; 4]);
    // The header's width says 9000 (its CRC is not checked by the size read).
    huge[16..20].copy_from_slice(&9000u32.to_be_bytes());
    let files: HashMap<String, Vec<u8>> = [
        ("huge.png", huge),
        ("broken.png", b"\x89PNG\r\n\x1a\nnot really".to_vec()),
        ("a.png", png_rgba(2, 2, &solid(2, 2, [0; 4]))),
        ("b.png", png_rgba(4, 4, &solid(4, 4, [0; 4]))),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let cases = [
        PbrMaterial {
            base: Some("gone.png".into()),
            ..PbrMaterial::default()
        },
        PbrMaterial {
            base: Some("c.tga".into()),
            ..PbrMaterial::default()
        },
        PbrMaterial {
            normal: Some("huge.png".into()),
            ..PbrMaterial::default()
        },
        PbrMaterial {
            base: Some("broken.png".into()),
            ..PbrMaterial::default()
        },
        PbrMaterial {
            roughness_map: Some("a.png".into()),
            metal_map: Some("b.png".into()),
            ..PbrMaterial::default()
        },
    ];
    for m in &cases {
        let mut textures = TextureTable::default();
        let mut read = |p: &str| files.get(p).cloned();
        assert!(load(m, "x", &mut read, &mut textures).is_err(), "{m:?}");
        assert!(textures.textures.is_empty());
    }
}

#[test]
fn a_draws_tangents_follow_its_u_direction() {
    let v = |p: [f32; 3], uv: [f32; 2]| HdVertex {
        position: p,
        normal: oct_encode([0.0, 0.0, 1.0]),
        uv,
        color: [255; 4],
        material: 0,
        extra: 0,
    };
    let vertices = [
        v([0.0, 0.0, 0.0], [0.0, 0.0]),
        v([0.0, 1.0, 0.0], [0.0, 1.0]),
        v([1.0, 0.0, 0.0], [1.0, 0.0]),
    ];
    let t = tangents_of(&vertices, &[0, 1, 2]);
    assert_eq!(t.len(), 3);
    for e in t {
        let d = super::super::oct_decode([e[0], e[1]]);
        assert!((d[0] - 1.0).abs() < 1e-3, "{d:?}");
        assert_eq!(e[2], 32767);
    }
}

#[test]
fn maps_over_the_working_memory_are_refused_from_their_headers() {
    // 4096 x 2048 three times (a side within the limit and a PNG height the decoder takes): over the material's working
    // memory together, refused before the first is decoded.
    let mut big = png_rgba(1, 1, &[0; 4]);
    big[16..20].copy_from_slice(&4096u32.to_be_bytes());
    big[20..24].copy_from_slice(&2048u32.to_be_bytes());
    assert!(working_bytes([(4096, 2048); 3]) > MAX_MATERIAL_BYTES);
    let m = PbrMaterial {
        base: Some("big.png".into()),
        normal: Some("big.png".into()),
        orm: Some("big.png".into()),
        ..PbrMaterial::default()
    };
    let mut textures = TextureTable::default();
    let mut read = |p: &str| (p == "big.png").then(|| big.clone());
    assert!(matches!(
        load(&m, "x", &mut read, &mut textures),
        Err(PbrLoadError::TooMuch(_))
    ));
}

#[test]
fn a_ktx2_normal_widening_an_image_orm_is_charged_its_working_memory_before_any_decode() {
    // A 4096 x 4096 BC5 normal with a PNG ORM header of 4096 x 2048: the files and the ORM alone fit the material's working
    // memory, the normal's decode and lengths on top do not. The PNG's pixels are missing, so a decode would fail another way.
    let normal = ktx2::write(
        &ktx2::Ktx2 {
            format: ktx2::Ktx2Format::Bc5,
            width: 4096,
            height: 4096,
            levels: vec![vec![0; 4096 * 4096]],
            key_values: Vec::new(),
        },
        ktx2::Supercompression::None,
    )
    .unwrap();
    let mut orm = png_rgba(1, 1, &[0; 4]);
    orm[16..20].copy_from_slice(&4096u32.to_be_bytes());
    orm[20..24].copy_from_slice(&2048u32.to_be_bytes());
    let normal_bytes = 4096 * 4096;
    assert!(
        working_bytes([(4096, 2048)]) + 2 * normal_bytes < MAX_MATERIAL_BYTES,
        "without the normal's own charge the load would reach the decode"
    );
    assert!(working_bytes([(4096, 2048), (4096, 4096)]) > MAX_MATERIAL_BYTES);
    let m = PbrMaterial {
        normal: Some("n.ktx2".into()),
        orm: Some("orm.png".into()),
        ..PbrMaterial::default()
    };
    let mut textures = TextureTable::default();
    let mut read = |p: &str| match p {
        "n.ktx2" => Some(normal.clone()),
        "orm.png" => Some(orm.clone()),
        _ => None,
    };
    assert!(matches!(
        load(&m, "x", &mut read, &mut textures),
        Err(PbrLoadError::TooMuch(_))
    ));
    assert!(textures.textures.is_empty());
}

#[test]
fn a_scene_of_many_large_maps_is_refused_past_its_texture_budget() {
    // Materials of one 4096 x 4096 map each: each keeps about 89 MB; past the budget a new one is refused.
    let mut held = 0u64;
    let mut admitted = 0;
    for _ in 0..256 {
        if over_budget(held, retained_bytes([(4096, 4096)])).is_some() {
            break;
        }
        held += retained_bytes([(4096, 4096)]);
        admitted += 1;
    }
    assert_eq!(admitted, 23);
    assert!(held <= MAX_SCENE_TEXTURE_BYTES);
}

#[test]
fn a_material_out_of_range_is_refused_before_its_maps_are_read() {
    let m = PbrMaterial {
        base: Some("../outside.png".into()),
        roughness: f32::NAN,
        ..PbrMaterial::default()
    };
    let mut reads = 0;
    let mut read = |_: &str| {
        reads += 1;
        None
    };
    let mut textures = TextureTable::default();
    assert!(matches!(
        load(&m, "x", &mut read, &mut textures),
        Err(PbrLoadError::Material(_))
    ));
    assert_eq!(reads, 0);
}

#[test]
fn a_map_of_a_huge_side_is_refused_before_any_memory_sum() {
    // A side of 2^30 squared is past u64 times the working bytes a texel: the side check comes first.
    let huge = MapImage {
        rgba: Vec::new(),
        width: 1 << 30,
        height: 1 << 30,
    };
    let maps = MapImages {
        base: Some(huge),
        ..MapImages::default()
    };
    let mut textures = TextureTable::default();
    assert!(matches!(
        build(&PbrMaterial::default(), "x", maps, &mut textures),
        Err(PbrLoadError::TooBig { .. })
    ));
    assert_eq!(working_bytes([(u32::MAX, u32::MAX); 8]), u64::MAX);
}

/// A bumpy 16 x 16 material's files: a color, a DirectX normal and an ORM map as PNGs.
fn bumpy_files() -> (PbrMaterial, HashMap<String, Vec<u8>>) {
    let color: Vec<u8> = (0..256u32)
        .flat_map(|i| [(i % 16 * 16) as u8, (i / 16 * 16) as u8, 90, 255])
        .collect();
    let normal: Vec<u8> = (0..256u32)
        .flat_map(|i| {
            if (i % 16 + i / 16) % 2 == 0 {
                [38, 128, 218, 255]
            } else {
                [217, 128, 218, 255]
            }
        })
        .collect();
    let files = [
        ("maps/wall_diff.png", png_rgba(16, 16, &color)),
        ("maps/wall_nor.png", png_rgba(16, 16, &normal)),
        (
            "maps/wall_arm.png",
            png_rgba(16, 16, &solid(16, 16, [255, 64, 0, 255])),
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let m = PbrMaterial {
        base: Some("maps/wall_diff.png".into()),
        normal: Some("maps/wall_nor.png".into()),
        normal_convention: NormalConvention::DirectX,
        orm: Some("maps/wall_arm.png".into()),
        ..PbrMaterial::default()
    };
    (m, files)
}

/// `m`'s maps baked from `files` and written beside them.
fn bake_beside(m: &PbrMaterial, files: &mut HashMap<String, Vec<u8>>) {
    let mut read = |p: &str| files.get(p).cloned();
    let baked = bake_files(m, &mut read).unwrap();
    assert_eq!(baked.len(), 3);
    for (path, b) in baked {
        files.insert(path, b.file);
    }
}

fn load_from(
    m: &PbrMaterial,
    files: &HashMap<String, Vec<u8>>,
) -> (Result<HdMaterial, PbrLoadError>, TextureTable) {
    let mut textures = TextureTable::default();
    let mut read = |p: &str| files.get(p).cloned();
    let got = load(m, "x", &mut read, &mut textures);
    (got, textures)
}

#[test]
fn baked_files_go_beside_their_first_map_named_by_role() {
    let (m, _) = bumpy_files();
    assert_eq!(
        baked_path(&m, MapRole::Base).as_deref(),
        Some("maps/wall_diff.base.ktx2")
    );
    assert_eq!(
        baked_path(&m, MapRole::Normal).as_deref(),
        Some("maps/wall_nor.normal.ktx2")
    );
    assert_eq!(baked_path(&m, MapRole::Height), None);
    let separate = PbrMaterial {
        roughness_map: Some("a.b/rough".into()),
        metal_map: Some("a.b/metal.jpg".into()),
        ..PbrMaterial::default()
    };
    assert_eq!(
        baked_path(&separate, MapRole::Orm).as_deref(),
        Some("a.b/rough.orm.ktx2")
    );
    let named = PbrMaterial {
        base: Some("wall.ktx2".into()),
        ..PbrMaterial::default()
    };
    assert_eq!(baked_path(&named, MapRole::Base), None);
}

#[test]
fn a_baked_material_loads_its_block_compressed_chains() {
    let (m, mut files) = bumpy_files();
    bake_beside(&m, &mut files);
    let (got, textures) = load_from(&m, &files);
    let got = got.unwrap();
    let format = |i: Option<usize>| textures.textures[i.unwrap()].format;
    use images::bc::BlockFormat as B;
    assert_eq!(format(got.base), TexelFormat::Block(B::Bc7));
    assert_eq!(format(got.normal), TexelFormat::Block(B::Bc5));
    assert_eq!(format(got.orm), TexelFormat::Block(B::Bc7));
    for t in &textures.textures {
        assert_eq!(t.levels.len(), 5);
        for (k, l) in t.levels.iter().enumerate() {
            assert_eq!(l.len() as u64, t.format.level_bytes(16 >> k, 16 >> k));
        }
    }
    // The ORM levels kept the normal map's lost length as roughness (Toksvig), as the uncompressed chain does.
    let orm = &textures.textures[got.orm.unwrap()];
    let decoded = images::bc::decode(B::Bc7, &orm.levels[1], 8, 8).unwrap();
    assert!(decoded[1] > 200, "{}", decoded[1]);
    // A quarter of the RGBA8 chains' bytes, but for the small levels' whole blocks.
    assert!(3 * textures.bytes() < retained_bytes([(16, 16); 3]));
}

#[test]
fn a_baked_file_of_other_sources_is_passed_over_for_the_maps() {
    let (m, mut files) = bumpy_files();
    bake_beside(&m, &mut files);
    // The color map changed after the bake: every map comes from the images again.
    files.insert(
        "maps/wall_diff.png".into(),
        png_rgba(16, 16, &solid(16, 16, [9, 9, 9, 255])),
    );
    let (got, textures) = load_from(&m, &files);
    assert!(
        textures
            .textures
            .iter()
            .all(|t| t.format == TexelFormat::Rgba8)
    );
    assert_eq!(
        &textures.textures[got.unwrap().base.unwrap()].levels[0][..4],
        &[9, 9, 9, 255]
    );
    // The settings count too: the same files as an OpenGL map.
    let (m, mut files) = bumpy_files();
    bake_beside(&m, &mut files);
    let gl = PbrMaterial {
        normal_convention: NormalConvention::OpenGl,
        ..m.clone()
    };
    let (got, textures) = load_from(&gl, &files);
    assert!(got.is_ok());
    assert!(
        textures
            .textures
            .iter()
            .all(|t| t.format == TexelFormat::Rgba8)
    );
    // Without the images, the baked files are what there is.
    files.retain(|k, _| k.ends_with(".ktx2"));
    let (got, textures) = load_from(&m, &files);
    assert!(got.is_ok());
    assert!(
        textures
            .textures
            .iter()
            .all(|t| t.format != TexelFormat::Rgba8)
    );
}

#[test]
fn a_sidecar_may_name_ktx2_maps_and_a_wrong_or_broken_one_fails_the_material() {
    let (m, mut files) = bumpy_files();
    bake_beside(&m, &mut files);
    let named = PbrMaterial {
        base: Some("maps/wall_diff.base.ktx2".into()),
        normal: Some("maps/wall_nor.normal.ktx2".into()),
        orm: Some("maps/wall_arm.png".into()),
        ..PbrMaterial::default()
    };
    let (got, textures) = load_from(&named, &files);
    let orm = &textures.textures[got.unwrap().orm.unwrap()];
    assert_eq!(orm.format, TexelFormat::Rgba8);
    // The ORM made here still widens its roughness by the KTX 2.0 normal's lengths.
    assert!(orm.levels[1][1] > 200, "{}", orm.levels[1][1]);
    let mut broken = files["maps/wall_diff.base.ktx2"].clone();
    broken.truncate(broken.len() - 3);
    files.insert("broken.ktx2".into(), broken);
    for base in ["maps/wall_nor.normal.ktx2", "broken.ktx2", "gone.ktx2"] {
        let m = PbrMaterial {
            base: Some(base.into()),
            ..PbrMaterial::default()
        };
        let (got, textures) = load_from(&m, &files);
        assert!(got.is_err(), "{base}");
        assert!(textures.textures.is_empty());
    }
}

#[test]
fn a_cutout_keeps_its_coverage_down_the_chain() {
    // A wavy disc of alpha 1 on 0: a straight average lets its edge fall under the cutoff level by level.
    let (w, cutoff) = (64u32, 0.5f32);
    let rgba: Vec<u8> = (0..w * w)
        .flat_map(|i| {
            let (x, y) = ((i % w) as f32 - 31.5, (i / w) as f32 - 31.5);
            let inside = (x * x + y * y).sqrt() < 9.0 + 6.0 * (x * 0.7).sin();
            [200, 200, 200, if inside { 255 } else { 0 }]
        })
        .collect();
    let share = |l: &[u8]| {
        let n = l.len() / 4;
        l.chunks(4)
            .filter(|p| f32::from(p[3]) / 255.0 >= cutoff)
            .count() as f32
            / n as f32
    };
    let kept = color_levels(&rgba, w, w, Some(cutoff));
    let level0 = share(&kept[0]);
    // Every level of 16 x 16 or more (below it one texel is more than 0.4 % of the level).
    for (k, l) in kept.iter().enumerate().take(3) {
        assert!(
            (share(l) - level0).abs() <= 0.02,
            "level {k}: {} vs {level0}",
            share(l)
        );
    }
}

#[test]
fn an_exr_height_map_loads_as_gray() {
    use exr::prelude::{
        AnyChannel, AnyChannels, Encoding, FlatSamples, Image, Layer, LayerAttributes, SmallVec,
        WritableImage,
    };
    let layer = Layer::new(
        (4, 4),
        LayerAttributes::default(),
        Encoding::default(),
        AnyChannels::sort(SmallVec::from_vec(vec![AnyChannel::new(
            "Y",
            FlatSamples::F32((0..16).map(|i| i as f32 / 15.0).collect()),
        )])),
    );
    let mut file = Vec::new();
    Image::from_layer(layer)
        .write()
        .to_buffered(std::io::Cursor::new(&mut file))
        .unwrap();
    let m = PbrMaterial {
        height: Some("disp.exr".into()),
        ..PbrMaterial::default()
    };
    let files: HashMap<String, Vec<u8>> = [("disp.exr".to_string(), file)].into();
    let (got, textures) = load_from(&m, &files);
    let level0 = &textures.textures[got.unwrap().height.unwrap()].levels[0];
    assert_eq!(&level0[..4], &[0, 0, 0, 255]);
    assert_eq!(&level0[60..64], &[255, 255, 255, 255]);
}

#[test]
fn sides_that_are_not_whole_blocks_bake_uncompressed() {
    let maps = MapImages {
        base: Some(MapImage {
            rgba: solid(6, 4, [1, 2, 3, 255]),
            width: 6,
            height: 4,
        }),
        ..MapImages::default()
    };
    let baked = bake(&PbrMaterial::default(), maps, None).unwrap();
    let t = ktx2::read(&baked[0].file, &ktx2::Limits::default()).unwrap();
    assert_eq!(t.format, Ktx2Format::Rgba8Srgb);
    assert_eq!(t.levels.len(), 3);
    assert!(t.key_values.iter().all(|(k, _)| k != STAMP_KEY));
}

#[test]
fn a_gray_map_file_is_its_gray_in_every_channel() {
    let files: HashMap<String, Vec<u8>> =
        [("disp.png".to_string(), png_of(2, 1, 0, &[40, 200]))].into();
    let m = PbrMaterial {
        height: Some("disp.png".into()),
        ..PbrMaterial::default()
    };
    let (got, textures) = load_from(&m, &files);
    let level0 = &textures.textures[got.unwrap().height.unwrap()].levels[0];
    assert_eq!(level0, &vec![40, 40, 40, 255, 200, 200, 200, 255]);
}
