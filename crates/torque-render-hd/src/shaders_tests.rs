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
