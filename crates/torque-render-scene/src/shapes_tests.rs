use super::*;

#[test]
fn bytes_round_as_round_does() {
    let mut v = 0.0f32;
    while v <= 255.0 {
        for x in [v, v + 0.5, v - 0.5, v + 0.49999997, v + 0.50000006] {
            let x = x.clamp(0.0, 255.0);
            assert_eq!(round_byte(x), x.round() as u8, "{x}");
        }
        v += 0.37;
    }
    assert_eq!(round_byte(f32::NAN), 0);
    assert_eq!(round_byte(255.0), 255);
}

fn triangles(kind: u32, idx: &[u16]) -> Vec<[u16; 3]> {
    let mut out = Vec::new();
    each_triangle(kind, idx, |t| out.push(t));
    out
}

#[test]
fn strips_alternate_their_winding_as_gl_does() {
    assert_eq!(
        triangles(PRIM_STRIP, &[0, 1, 2, 3, 4]),
        vec![[0, 1, 2], [2, 1, 3], [2, 3, 4]]
    );
    assert_eq!(
        triangles(PRIM_FAN, &[0, 1, 2, 3]),
        vec![[0, 1, 2], [0, 2, 3]]
    );
    assert_eq!(
        triangles(0, &[0, 1, 2, 3, 4, 5, 6]),
        vec![[0, 1, 2], [3, 4, 5]]
    );
}

#[test]
fn the_texture_folder_is_the_shapes() {
    assert_eq!(
        shape_dir("StaticShape 7 demo/data/shapes/tool.dts"),
        "demo/data/shapes"
    );
}

#[test]
fn set_material_toggles_blending_lighting_and_the_blend_func_only_on_a_change() {
    let mut gl = Gl::init();
    set_material(
        &mut gl,
        0,
        MAT_S_WRAP | MAT_T_WRAP | MAT_TRANSLUCENT | MAT_ADDITIVE,
        None,
    );
    assert!(gl.blend && !gl.depth_write && gl.texturing);
    assert_eq!(gl.blend_func, BlendFunc::Additive);
    set_material(&mut gl, 1, MAT_S_WRAP | MAT_SELF_ILLUMINATING, None);
    assert!(!gl.blend && gl.depth_write && !gl.lighting);
    assert_eq!(gl.blend_func, BlendFunc::Alpha, "the additive bit changed");
    // A colour with alpha under 1 ends with ONE, ZERO; a translucent material whose flags do not change keeps it.
    color_end(&mut gl, [1.0, 1.0, 1.0, 0.5]);
    assert_eq!(gl.blend_func, BlendFunc::Replace);
    set_material(
        &mut gl,
        2,
        MAT_S_WRAP | MAT_SELF_ILLUMINATING | MAT_TRANSLUCENT,
        None,
    );
    assert_eq!(gl.blend_func, BlendFunc::Replace);
    set_material(&mut gl, PRIM_NO_MATERIAL, 0, None);
    assert!(!gl.texturing && !gl.blend);
}

/// Files from a list.
struct Files(Vec<(String, Vec<u8>)>);

impl crate::Assets for Files {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0
            .iter()
            .find(|(p, _)| p.eq_ignore_ascii_case(path))
            .map(|(_, b)| b.clone())
    }
}

#[test]
fn a_synthetic_shape_at_rest_draws_the_same_each_build_and_loads_once() {
    let mut rng = shapes::synth::Rng(11);
    let model = shapes::synth::model(&mut rng, true, 4);
    let bytes = shapes::write::write_dts(&model);
    let files = Files(vec![("demo/data/shapes/synth.dts".into(), bytes)]);
    let none = |_: &str| None;
    clear_loaded();
    let build = || {
        let mut textures = TextureSet::new(&files);
        let mut notes = Vec::new();
        rest_draws(
            &none,
            &files,
            "demo/data/shapes/synth.dts",
            &mut textures,
            &mut notes,
        )
    };
    let (reads, _, parses) = loader_counts();
    let first = build().expect("the synthetic shape loads");
    let second = build().expect("and again");
    assert_eq!(
        first.0, second.0,
        "a shape at rest is a pure function of its file"
    );
    assert!(
        first.0.iter().all(|d| d.vertices.len() % 3 == 0),
        "triangle lists"
    );
    // (The counts are the process's: other tests load shapes meanwhile. The kept shapes are this thread's.)
    let (reads_after, _, parses_after) = loader_counts();
    assert!(parses_after > parses);
    assert!(reads_after > reads);
    assert_eq!(loaded_stats().0, 1, "the second build takes the kept shape");
    clear_loaded();
}

#[test]
fn a_missing_shape_draws_nothing() {
    let files = Files(Vec::new());
    let mut textures = TextureSet::new(&files);
    let none = |_: &str| None;
    assert!(
        rest_draws(
            &none,
            &files,
            "demo/nothing.dts",
            &mut textures,
            &mut Vec::new()
        )
        .is_none()
    );
}

/// TGE's flag bits (`TSMaterialList`): bits 0 to 8 and 27 to 31.
const TGE_BITS: [u32; 14] = [
    0x1,
    0x2,
    0x4,
    0x8,
    0x10,
    0x20,
    0x40,
    0x80,
    0x100,
    0x0800_0000,
    0x1000_0000,
    0x2000_0000,
    0x4000_0000,
    0x8000_0000,
];

#[test]
fn a_tge_material_samples_linear_whatever_its_flags() {
    // Every mix of TGE's bits, the nearest bit clear: the sampler TGE's `setMaterial` gives.
    for mix in 0u32..1 << TGE_BITS.len() {
        let flags = TGE_BITS
            .iter()
            .enumerate()
            .filter(|(k, _)| mix >> k & 1 != 0)
            .fold(0, |f, (_, b)| f | b);
        let s = material_sampler(flags);
        assert_eq!(
            s,
            Sampler {
                repeat: flags & (MAT_S_WRAP | MAT_T_WRAP) != 0,
                mag: Filter::Linear,
                min: Filter::Linear,
                mip: Filter::Linear,
                mipmaps: flags & MAT_NO_MIPMAP == 0,
            },
            "{flags:#x}"
        );
    }
    assert!(TGE_BITS.iter().all(|b| b & MAT_NEAREST == 0));
}

#[test]
fn a_nearest_material_samples_nearest_and_keeps_its_mipmaps() {
    let s = material_sampler(MAT_S_WRAP | MAT_T_WRAP | MAT_NEAREST);
    assert_eq!(
        (s.mag, s.min, s.mip, s.mipmaps, s.repeat),
        (Filter::Nearest, Filter::Nearest, Filter::Linear, true, true)
    );
    assert!(!material_sampler(MAT_NEAREST | MAT_NO_MIPMAP).mipmaps);
    // The interiors' nearest sampler is the same.
    assert_eq!(
        material_sampler(MAT_S_WRAP | MAT_NEAREST),
        crate::map::interior::NEAREST_SAMPLER
    );
}

#[test]
fn the_nearest_bit_alone_changes_the_sampler_and_nothing_else() {
    let mut gl = Gl::init();
    set_material(&mut gl, 0, MAT_S_WRAP | MAT_T_WRAP, Some(0));
    // A tint under 1 turns blending on; TGE keeps it while the flags do not change.
    gl.blend = true;
    set_material(&mut gl, 1, MAT_S_WRAP | MAT_T_WRAP | MAT_NEAREST, Some(0));
    assert!(gl.blend, "the nearest bit is no change of TGE's state");
    assert_eq!(gl.texture.map(|t| t.1.mag), Some(Filter::Nearest));
    set_material(&mut gl, 2, MAT_S_WRAP | MAT_T_WRAP, Some(0));
    assert_eq!(gl.texture.map(|t| t.1.mag), Some(Filter::Linear));
}

/// A textured box shape (material `tex`, `flags`) and its texture, its draws at rest.
fn box_draws(flags: u32) -> Vec<Draw> {
    use torque_shape_builder::{FULL_UV, MeshData, ShapeBuilder};
    let mut b = ShapeBuilder::new();
    let node = b.node("box", None, [0.0; 3]);
    let drawn = b.detail("detail1", 1.0);
    let tex = b.material("tex", flags);
    let mut mesh = MeshData::new(Some(tex));
    mesh.add_box([-0.5; 3], [0.5; 3], |_| FULL_UV);
    b.object("box", node, drawn, mesh);
    let bytes = shapes::write::write_dts(&b.build());
    let files = Files(vec![
        ("demo/box.dts".into(), bytes),
        (
            "demo/tex.png".into(),
            crate::textures::tests::png(4, 4, 200),
        ),
    ]);
    let none = |_: &str| None;
    clear_loaded();
    let mut textures = TextureSet::new(&files);
    let draws = rest_draws(
        &none,
        &files,
        "demo/box.dts",
        &mut textures,
        &mut Vec::new(),
    )
    .expect("the box loads")
    .0;
    clear_loaded();
    draws
}

#[test]
fn a_shape_written_with_the_nearest_bit_draws_nearest_and_one_without_draws_as_tge() {
    let samplers = |flags| {
        box_draws(flags)
            .iter()
            .map(|d| d.texture.expect("textured").1)
            .collect::<Vec<_>>()
    };
    let wrap = MAT_S_WRAP | MAT_T_WRAP;
    let nearest = samplers(wrap | MAT_NEAREST);
    assert!(!nearest.is_empty());
    assert!(
        nearest
            .iter()
            .all(|s| s.mag == Filter::Nearest && s.min == Filter::Nearest)
    );
    let plain = samplers(wrap);
    assert!(plain.iter().all(|s| *s == material_sampler(wrap)));
    // Apart from the sampler, the draws are the same.
    let strip = |mut d: Vec<Draw>| {
        for d in &mut d {
            d.texture = d.texture.map(|(t, _)| (t, material_sampler(0)));
        }
        d
    };
    assert_eq!(strip(box_draws(wrap | MAT_NEAREST)), strip(box_draws(wrap)));
}
