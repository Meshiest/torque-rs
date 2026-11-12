use super::*;

/// A small deterministic generator (xorshift64*).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn map(&mut self, name: &str) -> Option<String> {
        (self.below(3) > 0).then(|| format!("maps/{name}_{}.png", self.below(1000)))
    }
}

fn material(r: &mut Rng) -> PbrMaterial {
    PbrMaterial {
        base: r.map("base"),
        normal: r.map("normal"),
        normal_convention: if r.below(2) == 0 {
            NormalConvention::OpenGl
        } else {
            NormalConvention::DirectX
        },
        orm: r.map("orm"),
        occlusion: r.map("ao"),
        roughness_map: r.map("rough"),
        metal_map: r.map("metal"),
        height: r.map("height"),
        emissive: r.map("emit"),
        base_factor: [r.unit(), r.unit(), r.unit(), r.unit()],
        roughness: r.unit(),
        metal: r.unit(),
        ao_strength: r.unit(),
        normal_scale: r.unit() * 2.0,
        height_scale: r.unit() * 0.1,
        emissive_factor: [r.unit() * 10.0, r.unit(), 0.0],
        alpha: [AlphaMode::Opaque, AlphaMode::Cutout, AlphaMode::Blend][r.below(3)],
        alpha_cutoff: r.unit(),
        two_sided: r.below(2) == 0,
        uv_scale: [1.0 + r.unit() * 8.0, 0.5 + r.unit()],
    }
}

fn sidecar(r: &mut Rng, k: usize) -> Sidecar {
    if k.is_multiple_of(2) {
        Sidecar::One(Box::new(material(r)))
    } else {
        let n = 1 + r.below(6);
        Sidecar::Named(
            (0..n)
                .map(|i| (format!("mat_{k}_{i}"), material(r)))
                .collect(),
        )
    }
}

#[test]
fn forty_synthetic_material_sidecars_round_trip_with_no_difference() {
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    let mut differing = 0;
    for k in 0..40 {
        let s = sidecar(&mut r, k);
        let text = s.to_text();
        match Sidecar::parse(&text) {
            Ok(back) if back == s => {}
            other => {
                differing += 1;
                eprintln!("case {k}: {other:?}\n{text}");
            }
        }
    }
    assert_eq!(differing, 0);
}

#[test]
fn a_material_sidecar_of_either_form_reads_with_its_defaults() {
    let one = Sidecar::parse(
        r#"
base = "wall_color.png"
normal = "wall_normal_dx.png"
normal_convention = "directx"
orm = "wall_orm.png"
roughness = 0.8
uv_scale = [4.0, 4.0]
"#,
    )
    .expect("a single material");
    let m = one.material("anything").expect("the single material");
    assert_eq!(m.normal_convention, NormalConvention::DirectX);
    assert_eq!(m.roughness, 0.8);
    assert_eq!(m.metal, 1.0, "a factor left out is its default");
    assert_eq!(m.uv_scale, [4.0, 4.0]);
    let named = Sidecar::parse(
        r#"
[materials.body]
base = "body.png"
metal = 0.0
[materials.glass]
alpha = "blend"
base_factor = [0.9, 0.95, 1.0, 0.2]
roughness = 0.05
"#,
    )
    .expect("named materials");
    assert_eq!(named.iter().count(), 2);
    assert_eq!(
        named.material("glass").map(|m| m.alpha),
        Some(AlphaMode::Blend)
    );
    assert!(named.material("missing").is_none());
}

#[test]
fn sidecars_outside_the_limits_are_refused_not_read() {
    let refused = [
        ("absolute", "base = \"/etc/passwd\""),
        ("back", "base = \"../other/base.png\""),
        ("back inside", "base = \"maps/../../base.png\""),
        ("drive", "base = \"C:/base.png\""),
        ("backslash root", "base = \"\\\\server\\\\x.png\""),
        ("empty part", "base = \"maps//base.png\""),
        ("device", "base = \"NUL\""),
        ("device with an extension", "base = \"maps/con.png\""),
        ("numbered device", "normal = \"com3.jpg\""),
        ("printer", "orm = \"lpt1\""),
        ("superscript port", "base = \"COM\u{b9}.png\""),
        ("superscript printer", "height = \"maps/lpt\u{b3}\""),
        ("superscript in lower case", "normal = \"com\u{b2}.jpg\""),
        ("console input", "base = \"CONIN$\""),
        ("console output", "base = \"conout$.png\""),
        ("trailing dot", "base = \"base.png.\""),
        ("trailing space", "base = \"maps /base.png\""),
        ("nan", "roughness = nan"),
        ("infinite", "metal = inf"),
        ("over", "roughness = 1.5"),
        ("under", "uv_scale = [0.0, 1.0]"),
        ("unknown", "shininess = 3.0"),
        ("wrong type", "roughness = \"high\""),
        ("bad enum", "alpha = \"glass\""),
        (
            "named unknown",
            "[materials.a]\nbase = \"a.png\"\n[other]\nx = 1",
        ),
    ];
    for (what, text) in refused {
        assert!(Sidecar::parse(text).is_err(), "{what}: {text}");
    }
    let long = format!("base = \"{}\"", "a".repeat(MAX_SIDECAR_BYTES));
    assert!(matches!(
        Sidecar::parse(&long),
        Err(SidecarError::TooLong(_))
    ));
    let many: String = (0..=MAX_MATERIALS)
        .map(|i| format!("[materials.m{i}]\n"))
        .collect();
    assert!(matches!(
        Sidecar::parse(&many),
        Err(SidecarError::TooMany(_))
    ));
}

#[test]
fn sidecar_fuzz_300000_mutations_never_panic() {
    let mut r = Rng(0x1234_5678_9abc_def1);
    let seeds: Vec<Vec<u8>> = (0..16)
        .map(|k| sidecar(&mut r, k).to_text().into_bytes())
        .collect();
    let alphabet = b"[]=\".,#\n abc01-+einfnan_materials";
    let mut read = 0usize;
    for _ in 0..300_000 {
        let mut bytes = seeds[r.below(seeds.len())].clone();
        for _ in 0..1 + r.below(8) {
            if bytes.is_empty() {
                break;
            }
            let at = r.below(bytes.len());
            match r.below(4) {
                0 => bytes[at] = alphabet[r.below(alphabet.len())],
                1 => bytes[at] = r.next() as u8,
                2 => bytes.truncate(at),
                _ => bytes.insert(at, alphabet[r.below(alphabet.len())]),
            }
        }
        // Text that is not UTF-8 never reaches the parser (a file is read as a string first).
        if let Ok(text) = std::str::from_utf8(&bytes)
            && Sidecar::parse(text).is_ok()
        {
            read += 1;
        }
    }
    assert!(read > 0, "some mutations stay valid sidecars");
}

#[test]
fn a_files_sidecar_name_replaces_its_extension() {
    assert_eq!(sidecar_name("wall.png"), "wall.mat.toml");
    assert_eq!(sidecar_name("shapes/crate.dts"), "shapes/crate.mat.toml");
    assert_eq!(sidecar_name("dir.v2/plain"), "dir.v2/plain.mat.toml");
}

#[test]
fn plain_names_that_look_like_devices_are_read() {
    for path in [
        "console.png",
        "com10.png",
        "maps/nullify.jpg",
        "lpt.png",
        "a.b.png",
        "com\u{2074}.png",
        "conin.png",
        "lpt\u{b9}0.png",
    ] {
        let text = format!("base = \"{path}\"");
        assert!(Sidecar::parse(&text).is_ok(), "{path}");
    }
}
