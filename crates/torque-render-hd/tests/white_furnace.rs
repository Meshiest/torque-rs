//! The white furnace: spheres of albedo 1 under a uniform sky (the sky cube and the ambient both the same radiance, no sun, no
//! lights, no fog), at five roughnesses in five rows: reflective dielectrics and metals (the lobe reflects the sky cube), and
//! plain surfaces (the lobe reflects the ambient as a uniform sky) of dielectrics with all and with half of the highlights and of
//! metals. An energy-conserving BRDF gives back exactly the light that falls on it, so every pixel of every sphere must show the
//! sky's luminance (within 1 %, the half floats' rounding is under 0.5 %): a lobe sum that loses energy (rough metals without the
//! multi-scatter term) or adds it (a full diffuse under a full Fresnel lobe, or a diffuse that does not give up the share of a lobe
//! of half the highlights) shows as darker or brighter spheres.
//!
//! A test extension sets the spheres' surfaces from their material word's low bits: roughness (bits 0-6, hundredths), half the
//! highlights (bit 7), metalness (bit 8) and plain, reflecting no environment (bit 9). With `TORQUE_BRDF_GRID` naming a PNG path,
//! the ignored `sphere_grid_picture` draws the reflective rows in the sun under the physical sky over a floor and writes the
//! picture there (the look's sheet).

use torque_render_core::Gpu;
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{ExtCell, HdExtension, PlainScene};
use torque_render_hd::shaders::{NONE, ShaderExtension};
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::{
    DynamicObject, HdCamera, HdDraw, HdMode, HdScene, HdVertex, Quality, material, oct_encode,
};
use torque_render_scene::math::Mat;
use torque_render_scene::{Cull, Draw, TexEnv, Vertex};

const SIZE: (u32, u32) = (640, 320);
const FURNACE_SIZE: (u32, u32) = (640, 640);
/// The roughnesses of the cases (each drawn dielectric and metal).
const ROUGHNESS: [f32; 5] = [0.1, 0.3, 0.5, 0.75, 1.0];
/// The sky (gamma 0..1): the clear color the sky cube is drawn with, and the ambient.
const SKY: f32 = 0.6;
const RADIUS: f32 = 1.0;
/// The pixels within this share of a sphere's radius on screen are measured (its silhouette's pixels mix with the sky).
const INSIDE: f32 = 0.92;
const TOLERANCE: f32 = 0.01;

/// The test extension: the engine's chunks with an extension surface's roughness, highlights' share, metalness and reflection
/// from the material word (as the module's doc says), its F0 the dielectric's 0.04 or a metal's albedo.
struct Furnace;

fn furnace_shaders() -> ShaderExtension {
    static CHUNKS: std::sync::OnceLock<ShaderExtension> = std::sync::OnceLock::new();
    *CHUNKS.get_or_init(|| {
        let start = NONE.forward.find("fn ext_surface(").expect("ext_surface");
        let end = start + NONE.forward[start..].find('}').expect("its end") + 1;
        let forward = format!(
            "{}fn ext_surface(s: ptr<function, Surf>, material: u32, base: vec4<f32>, hollow: f32, translucent: bool) {{
    let metal = f32((material >> 8u) & 1u);
    (*s).rough = f32(material & 0x7fu) / 100.0;
    (*s).spec_level = select(1.0, 0.5, (material & 0x80u) != 0u);
    (*s).metal = metal;
    (*s).f0 = mix(vec3<f32>(0.04), base.rgb, metal);
    (*s).reflects = (material & 0x200u) == 0u;
}}{}",
            &NONE.forward[..start],
            &NONE.forward[end..]
        );
        ShaderExtension {
            common: NONE.common,
            forward: forward.leak(),
            shadow: NONE.shadow,
        }
    })
}

impl HdExtension for Furnace {
    type Geometry = StaticCells;
    type Scene = PlainScene;
    type CellRuns = ();
    type LevelInput = ();
    const FRAME_EXT_BYTES: usize = 16;

    fn new(_device: &torque_render_core::wgpu::Device) -> Furnace {
        Furnace
    }

    fn shaders() -> ShaderExtension {
        furnace_shaders()
    }

    fn mode(scene: &PlainScene) -> Option<HdMode> {
        scene.mode
    }

    fn time(scene: &PlainScene) -> f32 {
        scene.time
    }

    fn level_input(_cell: &ExtCell<Furnace>) {}
}

/// A case: its roughness, whether it is a metal, whether it reflects the environment (else the ambient as one), its share of
/// the highlights (1 or a half), and where its sphere stands.
#[derive(Debug, Clone, Copy)]
struct Case {
    rough: f32,
    metal: bool,
    reflects: bool,
    half_highlights: bool,
    center: [f32; 3],
}

/// The rows of cases, top down: reflective dielectrics and metals, then plain dielectrics with all and with half of the
/// highlights, and plain metals.
const ROWS: [(bool, bool, bool); 5] = [
    (false, true, false),
    (true, true, false),
    (false, false, false),
    (false, false, true),
    (true, false, false),
];

/// The cases, roughness rising to the right in each row.
fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    for (row, &(metal, reflects, half_highlights)) in ROWS.iter().enumerate() {
        for (k, &rough) in ROUGHNESS.iter().enumerate() {
            out.push(Case {
                rough,
                metal,
                reflects,
                half_highlights,
                center: [(k as f32 - 2.0) * 2.6, 0.0, 1.3 - row as f32 * 2.6],
            });
        }
    }
    out
}

/// A white sphere of `case`, an extension surface (kind 0) with its roughness and metalness in the material word.
fn sphere(case: Case, ghost: u32) -> DynamicObject {
    let (rings, segments) = (48u32, 96u32);
    let word = (case.rough * 100.0).round() as u32
        | (u32::from(case.half_highlights) << 7)
        | (u32::from(case.metal) << 8)
        | (u32::from(!case.reflects) << 9);
    let mut vertices = Vec::new();
    for i in 0..=rings {
        let theta = std::f32::consts::PI * i as f32 / rings as f32;
        for j in 0..=segments {
            let phi = std::f32::consts::TAU * j as f32 / segments as f32;
            let n = [
                theta.sin() * phi.cos(),
                theta.sin() * phi.sin(),
                theta.cos(),
            ];
            vertices.push(HdVertex {
                position: [0, 1, 2].map(|k| case.center[k] + RADIUS * n[k]),
                normal: oct_encode(n),
                uv: [0.0; 2],
                color: [255; 4],
                material: word,
                extra: 0,
            });
        }
    }
    let mut indices = Vec::new();
    for i in 0..rings {
        for j in 0..segments {
            let a = i * (segments + 1) + j;
            let b = a + segments + 1;
            indices.extend([a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    let mut draw = HdDraw::empty();
    draw.label = format!("sphere {ghost}");
    draw.vertices = vertices;
    draw.indices = indices;
    draw.cull = Cull::None;
    DynamicObject {
        ghost,
        transform: Mat::IDENTITY,
        prev_transform: Mat::IDENTITY,
        translucent: false,
        centre: case.center,
        radius: RADIUS,
        draws: vec![draw],
        previous: None,
        stamp: None,
        nodes: Vec::new(),
        takes_decals: false,
    }
}

/// The furnace: the spheres under the uniform sky, with nothing else lit or drawn (no AO: the BRDF alone is measured).
fn furnace_scene() -> PlainScene {
    let mut mode = HdMode::new(Quality::High);
    mode.image.physical_sky = false;
    mode.image.height_fog = false;
    mode.image.ao_strength = 0.0;
    mode.image.volumetric.scattering = 0.0;
    let mut s = PlainScene {
        frame: HdScene::new(StaticCells::default()),
        mode: Some(mode),
        time: 0.0,
    };
    s.clear = [SKY; 3];
    s.fog.color = [SKY; 3];
    s.fog.start = 1e5;
    s.fog.end = 2e5;
    s.visible_distance = 400.0;
    s.sun.diffuse = [0.0; 3];
    s.sun.ambient = [SKY; 3];
    s.sun.shadow = [0.0; 3];
    s.objects = cases()
        .into_iter()
        .enumerate()
        .map(|(k, c)| sphere(c, k as u32 + 1))
        .collect();
    s
}

fn camera(s: &mut PlainScene, eye: [f32; 3], at: [f32; 3], fov: u8, size: (u32, u32)) -> HdCamera {
    let e = look_at(eye, at);
    s.eye = Some(e);
    s.fov = fov;
    s.size = size;
    s.generation += 1;
    HdCamera::new(e, f32::from(fov), s.visible_distance, size.0, size.1)
}

/// The pixel (x, y down) where `p` is seen, and how many pixels a unit at `p` spans across the screen.
fn project(camera: &HdCamera, (w, h): (u32, u32), p: [f32; 3]) -> ([f32; 2], f32) {
    let m = camera.view_proj;
    let clip = [0, 1, 3].map(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r]);
    let (x, y) = (clip[0] / clip[2], clip[1] / clip[2]);
    // Clip y's row has the length of the projection's y scale (the view's turn keeps lengths).
    let focal = (m[1] * m[1] + m[5] * m[5] + m[9] * m[9]).sqrt();
    let scale = focal / clip[2] * 0.5 * h as f32;
    (
        [(x + 1.0) * 0.5 * w as f32, (1.0 - y) * 0.5 * h as f32],
        scale,
    )
}

fn luminance(c: [f32; 4]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// The linear value of a gamma 0..1 channel (the sRGB curve).
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// A case's sphere's pixels: their luminance over the sky's, (least, mean, most).
fn measured(
    hdr: &(u32, u32, Vec<[f32; 4]>),
    camera: &HdCamera,
    case: Case,
    sky: f32,
) -> (f32, f32, f32) {
    let (w, h, px) = hdr;
    let (center, scale) = project(camera, (*w, *h), case.center);
    let r = RADIUS * scale * INSIDE;
    let (mut lo, mut hi, mut sum, mut n) = (f32::INFINITY, 0.0f32, 0.0f64, 0u32);
    for y in (center[1] - r).floor().max(0.0) as u32..((center[1] + r).ceil() as u32).min(*h) {
        for x in (center[0] - r).floor().max(0.0) as u32..((center[0] + r).ceil() as u32).min(*w) {
            let (dx, dy) = (x as f32 + 0.5 - center[0], y as f32 + 0.5 - center[1]);
            if dx * dx + dy * dy > r * r {
                continue;
            }
            let l = luminance(px[(y * w + x) as usize]) / sky;
            lo = lo.min(l);
            hi = hi.max(l);
            sum += f64::from(l);
            n += 1;
        }
    }
    assert!(n > 100, "{case:?} covers {n} pixels");
    (lo, (sum / f64::from(n)) as f32, hi)
}

#[test]
fn spheres_of_albedo_one_under_a_uniform_sky_give_back_the_sky() {
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let mut s = furnace_scene();
    let camera = camera(
        &mut s,
        [0.0, -30.0, -3.9],
        [0.0, 0.0, -3.9],
        36,
        FURNACE_SIZE,
    );
    let mut world: HdWorld<Furnace> = HdWorld::new(&gpu);
    world.upload(&s);
    let target = gpu
        .offscreen(FURNACE_SIZE.0, FURNACE_SIZE.1)
        .expect("a target");
    let format = target.color.texture().format();
    for _ in 0..3 {
        world
            .draw(&s, &camera, Some((&target.color, format, [0, 0])))
            .expect("a frame");
    }
    let hdr = world.read_hdr().expect("the HDR target");
    let sky = srgb_to_linear(SKY);
    let mut failed = Vec::new();
    println!("case                                      least   mean    most");
    for case in cases() {
        let (lo, mean, hi) = measured(&hdr, &camera, case, sky);
        let name = format!(
            "{}{}{} rough {:.2}",
            if case.reflects {
                "reflective "
            } else {
                "plain "
            },
            if case.metal { "metal" } else { "dielectric" },
            if case.half_highlights { " (half)" } else { "" },
            case.rough
        );
        println!("{name:<42}{lo:.4}  {mean:.4}  {hi:.4}");
        if lo < 1.0 - TOLERANCE || hi > 1.0 + TOLERANCE {
            failed.push(name);
        }
    }
    assert!(
        failed.is_empty(),
        "outside {TOLERANCE} of the sky: {failed:?}"
    );
}

/// A floor for the picture (a plain plane of a mid gray).
fn floor() -> Draw {
    let half = 200.0;
    let corner = |x: f32, y: f32| Vertex {
        position: [x, y, -2.5],
        normal: [0.0, 0.0, 1.0],
        color: [128, 128, 128, 255],
        uv: [0.0; 2],
    };
    let (a, b, c, d) = (
        corner(-half, -half),
        corner(half, -half),
        corner(half, half),
        corner(-half, half),
    );
    Draw {
        label: "floor".into(),
        vertices: vec![a, b, c, a, c, d],
        lighting: true,
        tex_env: TexEnv::None,
        texture: None,
        blend: torque_render_scene::Blend::Opaque,
        cull: Cull::None,
        depth_test: true,
        depth_write: true,
        fog: true,
        lights: Vec::new(),
        vertex_key: None,
    }
}

#[test]
#[ignore = "writes the sphere grid's picture to TORQUE_BRDF_GRID"]
fn sphere_grid_picture() {
    let Some(path) = std::env::var_os("TORQUE_BRDF_GRID") else {
        eprintln!("skipped: TORQUE_BRDF_GRID names no path");
        return;
    };
    let gpu = Gpu::new().expect("a GPU");
    let mut s = furnace_scene();
    s.objects.truncate(2 * ROUGHNESS.len());
    if let Some(m) = &mut s.mode {
        m.image = HdMode::new(Quality::High).image;
        m.image.linear_light = std::env::var_os("TORQUE_BRDF_GAMMA").is_none();
    }
    s.atmosphere = true;
    s.clear = [0.55, 0.65, 0.8];
    s.sun.towards = [-0.5, -0.6, 0.62];
    s.sun.diffuse = [0.9, 0.86, 0.78];
    s.sun.ambient = [0.42, 0.45, 0.52];
    s.sun.shadow = [0.3, 0.33, 0.4];
    s.planes = vec![HdDraw::from_fixed(&floor(), material::KIND_PLANE, &[])];
    let camera = camera(&mut s, [3.0, -20.0, 5.0], [0.0; 3], 40, SIZE);
    let mut world: HdWorld<Furnace> = HdWorld::new(&gpu);
    world.reset_history();
    world.upload(&s);
    let target = gpu.offscreen(SIZE.0, SIZE.1).expect("a target");
    let format = target.color.texture().format();
    for _ in 0..16 {
        world
            .draw(&s, &camera, Some((&target.color, format, [0, 0])))
            .expect("a frame");
    }
    let frame = gpu.read_back(&target).expect("read back");
    let file = std::fs::File::create(path).expect("the picture's file");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), frame.width, frame.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut w| w.write_image_data(&frame.rgba))
        .expect("the picture is written");
}
