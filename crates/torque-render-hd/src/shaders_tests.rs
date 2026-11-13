use super::*;
use torque_render_scene::hd::material;

#[test]
fn every_shader_permutation_is_valid_wgsl() {
    let (ok, total, bad) = validate_all();
    assert_eq!(ok, total, "{bad:#?}");
}

#[test]
fn the_shaders_material_numbers_are_the_scenes() {
    // The forward pass's own constants (kept out of common.wgsl, which other modules take) are looked up there too.
    let forward = FILES
        .iter()
        .find(|f| f.0 == "forward.wgsl")
        .map_or("", |f| f.1);
    let common = format!(
        "{}
{forward}",
        FILES[0].1
    );
    for (name, value) in [
        ("MAT_GLOW", material::GLOW),
        ("MAT_UNLIT", material::UNLIT),
        ("MAT_ZERO_NORMAL", material::ZERO_NORMAL),
        ("MAT_TEX_ENV_SHIFT", material::TEX_ENV_SHIFT),
        ("MAT_KIND_SHIFT", material::KIND_SHIFT),
        ("KIND_PLANE", material::KIND_PLANE),
        ("KIND_SKY", material::KIND_SKY),
        ("KIND_SHAPE", material::KIND_SHAPE),
        ("MAT_EMITTER", material::EMITTER),
        ("MAT_CLOUD", material::CLOUD),
        ("MAT_PLAYER", material::PLAYER),
        ("MAT_BILLBOARD", material::BILLBOARD),
        ("MAT_PARTICLE", material::PARTICLE),
        ("MAT_DECAL", material::DECAL),
        ("MAT_LIT", material::LIT),
        ("MAT_ANCHORED", material::ANCHORED),
        ("MAT_SKY_EYE", material::SKY_EYE),
        ("MAT_NODE", material::NODE),
        ("MAT_LAYER", material::LAYER),
        ("MAT_LIGHTMAP", material::LIGHTMAP),
        ("MAT_CUTOUT", material::CUTOUT),
    ] {
        let line = common
            .lines()
            .find(|l| l.starts_with(&format!("const {name}: u32 = ")))
            .unwrap_or_else(|| panic!("{name} is not in common.wgsl"));
        let v = line
            .split('=')
            .nth(1)
            .unwrap()
            .trim()
            .trim_end_matches(';')
            .trim_end_matches('u');
        let parsed = match v.strip_prefix("0x") {
            Some(h) => u32::from_str_radix(h, 16).unwrap(),
            None => v.parse().unwrap(),
        };
        assert_eq!(parsed, value, "{name}");
    }
}

/// A file's `const NAME: f32 = value;` (or u32) as a number.
fn constant(file: &str, name: &str) -> f64 {
    let text = FILES.iter().find(|(n, _)| *n == file).unwrap().1;
    let line = text
        .lines()
        .find(|l| l.starts_with(&format!("const {name}: ")))
        .unwrap_or_else(|| panic!("{name} is not in {file}"));
    line.split('=')
        .nth(1)
        .unwrap()
        .trim()
        .trim_end_matches(';')
        .trim_end_matches('u')
        .parse()
        .unwrap()
}

#[test]
fn the_terrain_layers_sharpening_is_the_scenes() {
    let common = FILES[0].0;
    assert_eq!(
        constant(common, "LAYER_SHARPNESS") as f32,
        material::LAYER_SHARPNESS
    );
    assert_eq!(constant(common, "LAYER_DIP") as f32, material::LAYER_DIP);
}

#[test]
fn the_grading_tables_side_is_the_scenes() {
    assert_eq!(
        constant("tonemap.wgsl", "LUT_N") as usize,
        torque_render_scene::hd::lut::LUT_SIZE
    );
}

#[test]
fn the_area_lights_table_side_is_the_scenes() {
    assert_eq!(
        constant("area.wgsl", "LTC_SIZE") as usize,
        torque_render_scene::hd::ltc::LTC_SIZE
    );
}

#[test]
fn the_sky_and_tonemap_constants_are_the_scenes() {
    use torque_render_scene::hd::{atmosphere as a, grade as g};
    for (name, value) in [
        ("TRANSMITTANCE_W", f64::from(a::TRANSMITTANCE_SIZE.0)),
        ("TRANSMITTANCE_H", f64::from(a::TRANSMITTANCE_SIZE.1)),
        ("TRANSMITTANCE_STEPS", f64::from(a::TRANSMITTANCE_STEPS)),
        ("MULTISCATTER_N", f64::from(a::MULTISCATTER_SIZE)),
        (
            "MULTISCATTER_DIRECTIONS",
            f64::from(a::MULTISCATTER_DIRECTIONS),
        ),
        ("MULTISCATTER_STEPS", f64::from(a::MULTISCATTER_STEPS)),
        ("SKY_VIEW_W", f64::from(a::SKY_VIEW_SIZE.0)),
        ("SKY_VIEW_H", f64::from(a::SKY_VIEW_SIZE.1)),
        ("SKY_VIEW_STEPS", f64::from(a::SKY_VIEW_STEPS)),
        ("MULTISCATTER_CROWD", a::MULTISCATTER_CROWD),
    ] {
        assert_eq!(constant("skycommon.wgsl", name), value, "{name}");
    }
    for (name, value) in [
        ("AGX_INSET", g::AGX_INSET),
        ("AGX_MIN_EV", g::AGX_MIN_EV),
        ("AGX_MAX_EV", g::AGX_MAX_EV),
        ("AGX_SLOPE", g::AGX_SLOPE),
        ("AGX_MID_GREY_DISPLAY", g::AGX_MID_GREY_DISPLAY),
        ("AGX_OUTSET", g::AGX_OUTSET),
    ] {
        assert_eq!(constant("tonemap.wgsl", name) as f32, value, "{name}");
    }
    // The sun disk's own path to white starts at the tonemap's shoulder.
    use torque_render_scene::hd::sky as s;
    assert_eq!(
        constant("tonemap.wgsl", "SHOULDER") as f32,
        s::DISK_SHOULDER
    );
    for (name, value) in [
        ("DISK_SHOULDER", s::DISK_SHOULDER),
        ("DISK_WHITE_SPILL", s::DISK_WHITE_SPILL),
    ] {
        assert_eq!(constant("sky.wgsl", name) as f32, value, "{name}");
    }
}

#[test]
fn the_sun_rays_constants_are_the_scenes() {
    use torque_render_scene::hd::clouds as c;
    for (name, value) in [
        ("SUN_RAYS", c::SUN_RAYS),
        ("SUN_SEGMENTS", c::SUN_SEGMENTS),
        ("SUN_SEGMENT_STEPS", c::SUN_SEGMENT_STEPS),
        ("REFINED_STEPS", c::REFINED_STEPS),
    ] {
        assert_eq!(constant("clouds.wgsl", name) as u32, value, "{name}");
    }
}

#[test]
fn the_cloud_cubes_uniform_places_and_mips_are_the_shaders() {
    use crate::clouds::{
        CLOUD_CUBE_MIPS, CLOUD_CUBE_SIZE, LAYER_VEC4S, LOD_VEC4, MARCH_VEC4, UPPER_SCALE,
        UPPER_VEC4,
    };
    use torque_render_scene::hd::clouds::MAX_LAYERS;
    use torque_render_scene::hd::reflect::CAPTURE_SIZE;
    // The cube's march changes the clouds' uniform's `march`, `lod` and `upper` (clouds.rs): their places in clouds.wgsl's
    // struct, the layers first and then a vec4 a field.
    let text = FILES.iter().find(|(n, _)| *n == "clouds.wgsl").unwrap().1;
    let body = text
        .split("struct Clouds {")
        .nth(1)
        .and_then(|b| b.split("};").next())
        .unwrap();
    let fields: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .filter_map(|l| l.split(':').next())
        .collect();
    assert_eq!(fields[0], "layers");
    let at = |name: &str| {
        let k = fields.iter().position(|f| *f == name).unwrap();
        MAX_LAYERS * LAYER_VEC4S + k - 1
    };
    assert_eq!(at("march"), MARCH_VEC4);
    assert_eq!(at("lod"), LOD_VEC4);
    assert_eq!(at("upper"), UPPER_VEC4);
    // The upper volumes' image is made at the size the march reads it at.
    assert_eq!(
        constant("clouds.wgsl", "UPPER_SCALE"),
        f64::from(UPPER_SCALE)
    );
    // reflect.wgsl reads the cube's mips as many levels down from the sky cube's as the sky cube is wider, and the cube has
    // the mips its reads take (the water's clouds and the reflections' fog).
    let shift = f64::from((CAPTURE_SIZE / CLOUD_CUBE_SIZE).ilog2());
    assert_eq!(constant("reflect.wgsl", "CLOUD_CUBE_SHIFT"), shift);
    for (file, name) in [
        ("water.wgsl", "REFLECTED_CLOUD_CUBE_LOD"),
        ("skyfog.wgsl", "FOG_CUBE_LOD"),
    ] {
        let level = constant(file, name) - shift;
        assert!(
            (0.0..f64::from(CLOUD_CUBE_MIPS)).contains(&level),
            "{name}: level {level}"
        );
    }
}

#[test]
fn only_the_dissolve_permutations_take_its_chunks_and_their_entries_are_plain_functions_there() {
    for p in Permutation::ALL {
        let src = p.source();
        let own = matches!(
            p,
            Permutation::Dissolve { .. } | Permutation::ShadowDissolve { .. }
        );
        assert_eq!(src.contains("DissolveSlot"), own, "{}", p.name());
        if let Permutation::Dissolve { clustered } = p {
            for (entry, plain) in DISSOLVE_RENAMES {
                assert!(!src.contains(entry) && src.contains(plain), "{entry}");
            }
            // Everything else is the opaque pass's module as it is.
            let mut forward = Permutation::Forward {
                clustered,
                front_layer: false,
            }
            .source();
            for (entry, plain) in DISSOLVE_RENAMES {
                forward = forward.replacen(entry, plain, 1);
            }
            assert!(src.starts_with(&forward));
        }
    }
}

/// The CPU's field ([`torque_render_scene::hd::dissolve::field`]) is the shader's: the same constants in the same order.
#[test]
fn the_cpu_field_is_the_shaders() {
    let wgsl = FILES
        .iter()
        .find(|(n, _)| *n == "dissolve_common.wgsl")
        .map(|(_, t)| *t)
        .expect("the file");
    for c in [
        "0x8da6b343u",
        "0xd8163841u",
        "0xcb1ab31fu",
        "0x7feb352du",
        "0x846ca68bu",
        "16777216.0",
        "q * 2.03 + vec3<f32>(17.1)",
        "q * 4.01 + vec3<f32>(31.7)",
        "* 0.5 +",
        ") * 0.3",
        ") * 0.2",
    ] {
        assert!(wgsl.contains(c), "{c}");
    }
}

/// Every permutation that draws without a PBR material, by name, and the FNV-1a hash of its source composed with the engine's
/// own extension: a change to any of them moves pixels that must not move (the PBR path has modules of its own).
/// The forward, water, dissolve, sprite and particle entries changed their hashes for the round small sprites and the bounce (the
/// forward fragment tests a pool sprite's footprint, zero for every other draw); `shots-diff` against main's pictures showed no
/// other pixel move. The forward entries changed again when the projected decals moved the prepass's id to a `vec2`; the level
/// shots matched main's exactly. The froxels entry changed when a froxel's lights were taken at points along its depth; the
/// `fog-lights` pictures drawn without the grid or without point lights matched main's exactly.
const NON_PBR_SOURCES: [(&str, u64); 41] = [
    (
        "forward.wgsl (CLUSTERED = true, FRONT_LAYER = false)",
        0x6b8bf5d77577880b,
    ),
    (
        "forward.wgsl (CLUSTERED = false, FRONT_LAYER = false)",
        0x2aa7de84d9c03ac0,
    ),
    (
        "forward.wgsl (CLUSTERED = true, FRONT_LAYER = true)",
        0x80e914678aa10b2e,
    ),
    (
        "forward.wgsl (CLUSTERED = false, FRONT_LAYER = true)",
        0x9503d6f6076b8393,
    ),
    ("clusters.wgsl", 0xf9910b500be3f387),
    ("tonemap.wgsl", 0xaf3880f9b042906f),
    ("shadow.wgsl (UNCLIPPED = true)", 0x2c77f938a82871e2),
    ("shadow.wgsl (UNCLIPPED = false)", 0x63ae3f4de11ba8b1),
    ("contact.wgsl", 0xf272e766dd6b3f38),
    ("contact_filter.wgsl", 0x5a720895d77e42c9),
    ("ao.wgsl (GTAO = true)", 0x4d5308986eebf3e7),
    ("ao.wgsl (GTAO = false)", 0x44bb2e6b6ddd22bc),
    ("ao_filter.wgsl", 0xd665528f97e9c439),
    ("taa.wgsl", 0x8a6e99cfb267243a),
    ("fxaa.wgsl", 0xa96525dca2b78bff),
    (
        "forward.wgsl (a secondary view: CLUSTERED = false, CLIP = true)",
        0x3cd1975ebba2495b,
    ),
    ("water.wgsl", 0x092b2cec714e1ae1),
    ("hiz.wgsl", 0x0b2dc8f0063114ed),
    ("ssr.wgsl", 0x736fc1e3bce41d02),
    ("down.wgsl", 0x3d4a40d1d9c5b930),
    ("prefilter.wgsl", 0xaebb7728517df56a),
    ("irradiance.wgsl", 0x62c652196617e311),
    ("atmosphere.wgsl", 0x39c9bd8b08ea6c07),
    ("sky.wgsl", 0x989fd0754a5928a0),
    ("volumetric.wgsl", 0xdfd7672a3a7fc3f6),
    ("froxels.wgsl", 0xfd4404abe73aed6b),
    ("bloom.wgsl", 0x6f4fbe1ca8643292),
    ("exposure.wgsl", 0x7e42d0828fe4962f),
    ("wboit.wgsl", 0x202ec555530680d0),
    ("clouds.wgsl", 0x93e82945bd675318),
    ("cull.wgsl", 0x0ff6b6076072b463),
    (
        "forward.wgsl with gpu_sprite.wgsl (CLUSTERED = true, FRONT_LAYER = false)",
        0x50675895a8c5b8a0,
    ),
    (
        "forward.wgsl with gpu_sprite.wgsl (CLUSTERED = false, FRONT_LAYER = false)",
        0x03b5750c21ddc4b9,
    ),
    (
        "forward.wgsl with gpu_sprite.wgsl (CLUSTERED = true, FRONT_LAYER = true)",
        0x1ce58e3b86220187,
    ),
    (
        "forward.wgsl with gpu_sprite.wgsl (CLUSTERED = false, FRONT_LAYER = true)",
        0x58507470beb1ab28,
    ),
    ("gpu_particles.wgsl", 0x4f73054f871a31ee),
    (
        "forward.wgsl with dissolve.wgsl (CLUSTERED = true)",
        0x295c6778db371af9,
    ),
    (
        "forward.wgsl with dissolve.wgsl (CLUSTERED = false)",
        0xa2df584679a09586,
    ),
    (
        "shadow.wgsl with dissolve_shadow.wgsl (UNCLIPPED = true)",
        0x8dd6d7c0d9573073,
    ),
    (
        "shadow.wgsl with dissolve_shadow.wgsl (UNCLIPPED = false)",
        0xe5b1ff61bdf5fc90,
    ),
    ("forward.wgsl with decals.wgsl", 0xb816b66574d7223b),
];

#[test]
fn the_permutations_without_pbr_materials_are_unchanged() {
    let fnv = |text: &str| {
        text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
    };
    let changed: Vec<String> = NON_PBR_SOURCES
        .iter()
        .filter(|(name, hash)| {
            Permutation::ALL
                .iter()
                .find(|p| p.name() == *name)
                .is_none_or(|p| fnv(&p.source()) != *hash)
        })
        .map(|(name, _)| (*name).to_string())
        .collect();
    assert_eq!(changed, Vec::<String>::new());
}

/// The projected decals' clip cosine and facing fade are the scene crate's, and their module is the opaque pass's forward module
/// with its entries after it (every other module is as without them).
#[test]
fn the_decal_shaders_constants_are_the_scene_crates() {
    use torque_render_scene::decals::CLIP_ANGLE_COS;
    use torque_render_scene::hd::decals::FACING_FADE;
    assert_eq!(
        constant("decals.wgsl", "DECAL_CLIP_COS") as f32,
        CLIP_ANGLE_COS
    );
    assert_eq!(
        constant("decals.wgsl", "DECAL_FACING_FADE") as f32,
        FACING_FADE
    );
    let forward = Permutation::Forward {
        clustered: true,
        front_layer: false,
    }
    .source();
    for p in Permutation::ALL {
        let own = p == Permutation::Decals;
        assert_eq!(p.source().contains("fn vs_decal("), own, "{}", p.name());
    }
    assert!(Permutation::Decals.source().starts_with(&forward));
}
