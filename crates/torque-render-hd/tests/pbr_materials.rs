//! The physically based materials' path on the GPU.
//!
//! - The sweep: white spheres of materials with no maps, roughness 0.1 to 1 by metalness 0 and 1, under a uniform sky (the
//!   sky cube and the ambient the same radiance, no sun, no lights, no fog). The reference integral of an energy-conserving
//!   BRDF of albedo 1 over a uniform sky is the sky itself, so every pixel of every sphere must show the sky's luminance within
//!   2 %. A row of metalness 0.5 is drawn and printed, not held to it: F0 mixed toward the albedo and the diffuse times
//!   (1 - metalness), the metallic workflow's usual blend, gives back about 70-75 % there (the two lobes blended by the
//!   metalness, not their mixed Fresnel, would conserve it).
//! - Maps: a normal map turned toward the sun lights a floor more than a flat one, and an ORM map's roughness and metalness
//!   change what the floor reflects.
//! - Nothing else moves: a scene's draws without a material draw the same bytes whether or not the scene holds materials.

use torque_modelcore::pbr::PbrMaterial;
use torque_render_core::Gpu;
use torque_render_hd::HdWorld;
use torque_render_hd::ext::PlainScene;
use torque_render_scene::Cull;
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::pbr::{self, HdMaterial, MapImage, MapImages, MaterialId, PbrDraw};
use torque_render_scene::hd::{
    DynamicObject, HdCamera, HdDraw, HdMode, HdScene, HdVertex, Quality, material, oct_encode,
};
use torque_render_scene::math::Mat;

const SIZE: (u32, u32) = (640, 400);
const ROUGHNESS: [f32; 5] = [0.1, 0.3, 0.5, 0.75, 1.0];
const METAL: [f32; 3] = [0.0, 0.5, 1.0];
const SKY: f32 = 0.6;
const RADIUS: f32 = 1.0;
const INSIDE: f32 = 0.92;
const TOLERANCE: f32 = 0.02;

/// A UV sphere about `center`: its vertices (material word of a shape), indices, and tangents.
fn sphere(center: [f32; 3], radius: f32) -> (Vec<HdVertex>, Vec<u32>) {
    let (rings, segments) = (48u32, 96u32);
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
                position: [0, 1, 2].map(|k| center[k] + radius * n[k]),
                normal: oct_encode(n),
                uv: [2.0 * j as f32 / segments as f32, i as f32 / rings as f32],
                color: [255; 4],
                material: material::KIND_SHAPE << material::KIND_SHIFT,
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
    (vertices, indices)
}

/// An object of one draw, with `material` (and its tangents) when given.
fn object(
    ghost: u32,
    center: [f32; 3],
    radius: f32,
    draw: (Vec<HdVertex>, Vec<u32>),
    material: Option<MaterialId>,
) -> DynamicObject {
    let mut d = HdDraw::empty();
    d.label = format!("object {ghost}");
    d.pbr = material.map(|m| PbrDraw {
        material: m,
        tangents: pbr::tangents_of(&draw.0, &draw.1),
    });
    d.vertices = draw.0;
    d.indices = draw.1;
    d.cull = Cull::None;
    DynamicObject {
        ghost,
        transform: Mat::IDENTITY,
        prev_transform: Mat::IDENTITY,
        translucent: false,
        centre: center,
        radius,
        draws: vec![d],
        previous: None,
        stamp: None,
        nodes: Vec::new(),
        takes_decals: false,
    }
}

fn uniform_sky_scene() -> PlainScene {
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
    s
}

fn camera(s: &mut PlainScene, eye: [f32; 3], at: [f32; 3], fov: u8) -> HdCamera {
    let e = look_at(eye, at);
    s.eye = Some(e);
    s.fov = fov;
    s.size = SIZE;
    s.generation += 1;
    HdCamera::new(e, f32::from(fov), s.visible_distance, SIZE.0, SIZE.1)
}

fn project(camera: &HdCamera, (w, h): (u32, u32), p: [f32; 3]) -> ([f32; 2], f32) {
    let m = camera.view_proj;
    let clip = [0, 1, 3].map(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r]);
    let (x, y) = (clip[0] / clip[2], clip[1] / clip[2]);
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

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The pixels of the disk of `center` and `radius` on screen: their luminance over `sky` (least, mean, most).
fn measured(
    hdr: &(u32, u32, Vec<[f32; 4]>),
    camera: &HdCamera,
    center: [f32; 3],
    sky: f32,
) -> (f32, f32, f32) {
    let (w, h, px) = hdr;
    let (c, scale) = project(camera, (*w, *h), center);
    let r = RADIUS * scale * INSIDE;
    let (mut lo, mut hi, mut sum, mut n) = (f32::INFINITY, 0.0f32, 0.0f64, 0u32);
    for y in (c[1] - r).floor().max(0.0) as u32..((c[1] + r).ceil() as u32).min(*h) {
        for x in (c[0] - r).floor().max(0.0) as u32..((c[0] + r).ceil() as u32).min(*w) {
            let (dx, dy) = (x as f32 + 0.5 - c[0], y as f32 + 0.5 - c[1]);
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
    assert!(n > 100, "{center:?} covers {n} pixels");
    (lo, (sum / f64::from(n)) as f32, hi)
}

fn gpu() -> Option<Gpu> {
    match Gpu::new() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

fn draw_frames(gpu: &Gpu, s: &PlainScene, camera: &HdCamera, frames: usize) -> HdWorld {
    let mut world: HdWorld = HdWorld::new(gpu);
    world.upload(s);
    let target = gpu.offscreen(SIZE.0, SIZE.1).expect("a target");
    let format = target.color.texture().format();
    for _ in 0..frames {
        world
            .draw(s, camera, Some((&target.color, format, [0, 0])))
            .expect("a frame");
    }
    world
}

#[test]
fn a_roughness_by_metalness_sweep_gives_back_the_uniform_sky_within_two_percent() {
    let Some(gpu) = gpu() else {
        return;
    };
    let mut s = uniform_sky_scene();
    let mut centers = Vec::new();
    for (row, &metal) in METAL.iter().enumerate() {
        for (k, &rough) in ROUGHNESS.iter().enumerate() {
            let id = s.materials.intern(HdMaterial::of_factors(&PbrMaterial {
                roughness: rough,
                metal,
                ..PbrMaterial::default()
            }));
            let center = [(k as f32 - 2.0) * 2.6, 0.0, 2.6 - row as f32 * 2.6];
            let ghost = (row * ROUGHNESS.len() + k) as u32 + 1;
            s.objects.push(object(
                ghost,
                center,
                RADIUS,
                sphere(center, RADIUS),
                Some(id),
            ));
            centers.push((metal, rough, center));
        }
    }
    let cam = camera(&mut s, [0.0, -26.0, 0.0], [0.0; 3], 36);
    let world = draw_frames(&gpu, &s, &cam, 3);
    let hdr = world.read_hdr().expect("the HDR target");
    let sky = srgb_to_linear(SKY);
    let mut failed = Vec::new();
    println!("metal rough   least   mean    most");
    for (metal, rough, center) in centers {
        let (lo, mean, hi) = measured(&hdr, &cam, center, sky);
        println!("{metal:.2}  {rough:.2}   {lo:.4}  {mean:.4}  {hi:.4}");
        let held = metal == 0.0 || metal == 1.0;
        if held && (lo < 1.0 - TOLERANCE || hi > 1.0 + TOLERANCE) {
            failed.push((metal, rough));
        }
    }
    assert!(
        failed.is_empty(),
        "outside {TOLERANCE} of the sky: {failed:?}"
    );
}

/// A floor quad of `size` about the origin with UVs of one repeat, its material.
fn floor(size: f32, uv: f32) -> (Vec<HdVertex>, Vec<u32>) {
    let corner = |x: f32, y: f32| HdVertex {
        position: [x * size, y * size, 0.0],
        normal: oct_encode([0.0, 0.0, 1.0]),
        uv: [(x + 1.0) * 0.5 * uv, (1.0 - y) * 0.5 * uv],
        color: [255; 4],
        material: material::KIND_SHAPE << material::KIND_SHIFT,
        extra: 0,
    };
    (
        vec![
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ],
        vec![0, 2, 1, 0, 3, 2],
    )
}

fn map(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> MapImage {
    MapImage {
        rgba: (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .flat_map(|(x, y)| f(x, y))
            .collect(),
        width: w,
        height: h,
    }
}

/// The floor in a low sun from +X under a dim sky, of `maps`: its mean luminance at the screen's middle.
fn lit_floor(gpu: &Gpu, maps: MapImages, rough: f32, metal: f32) -> f32 {
    let m = PbrMaterial {
        roughness: rough,
        metal,
        ..PbrMaterial::default()
    };
    lit_floor_of(gpu, |s| {
        pbr::build(&m, "floor", maps, &mut s.textures).expect("the maps")
    })
}

/// The floor of the material `make` puts in the scene's tables, lit as [`lit_floor`]'s.
fn lit_floor_of(gpu: &Gpu, make: impl FnOnce(&mut PlainScene) -> HdMaterial) -> f32 {
    let mut s = uniform_sky_scene();
    s.clear = [0.3; 3];
    s.sun.ambient = [0.2; 3];
    s.sun.diffuse = [0.8; 3];
    s.sun.towards = [0.8, 0.0, 0.6];
    let built = make(&mut s);
    let id = s.materials.intern(built);
    s.objects
        .push(object(1, [0.0; 3], 6.0, floor(6.0, 4.0), Some(id)));
    let cam = camera(&mut s, [0.0, -6.0, 5.0], [0.0, 0.0, 0.0], 40);
    let world = draw_frames(gpu, &s, &cam, 3);
    let (w, h, px) = world.read_hdr().expect("the HDR target");
    let mut sum = 0.0;
    let mut n = 0;
    for y in h / 2 - 20..h / 2 + 20 {
        for x in w / 2 - 40..w / 2 + 40 {
            sum += luminance(px[(y * w + x) as usize]);
            n += 1;
        }
    }
    sum / n as f32
}

#[test]
fn a_normal_map_turned_to_the_sun_lights_more_and_an_orm_map_changes_the_reflection() {
    let Some(gpu) = gpu() else {
        return;
    };
    let white = || Some(map(8, 8, |_, _| [200, 200, 200, 255]));
    let flat = lit_floor(
        &gpu,
        MapImages {
            base: white(),
            ..MapImages::default()
        },
        0.8,
        0.0,
    );
    // Normals tilted 40 degrees toward +X (the tangent: U runs along +X).
    let (x, z) = (40f32.to_radians().sin(), 40f32.to_radians().cos());
    let enc = |v: f32| ((v * 0.5 + 0.5) * 255.0).round() as u8;
    let toward = lit_floor(
        &gpu,
        MapImages {
            base: white(),
            normal: Some(map(8, 8, |_, _| [enc(x), 128, enc(z), 255])),
            ..MapImages::default()
        },
        0.8,
        0.0,
    );
    let away = lit_floor(
        &gpu,
        MapImages {
            base: white(),
            normal: Some(map(8, 8, |_, _| [enc(-x), 128, enc(z), 255])),
            ..MapImages::default()
        },
        0.8,
        0.0,
    );
    println!("flat {flat:.4}, toward the sun {toward:.4}, away {away:.4}");
    assert!(toward > flat * 1.1, "toward {toward} flat {flat}");
    assert!(away < flat * 0.9, "away {away} flat {flat}");
    // An ORM map of metalness 1 makes the floor a mirror of its white albedo: not the dielectric's look.
    let metal = lit_floor(
        &gpu,
        MapImages {
            base: white(),
            orm: Some(map(8, 8, |_, _| [255, 255, 255, 255])),
            ..MapImages::default()
        },
        0.3,
        1.0,
    );
    let plastic = lit_floor(
        &gpu,
        MapImages {
            base: white(),
            orm: Some(map(8, 8, |_, _| [255, 255, 0, 255])),
            ..MapImages::default()
        },
        0.3,
        1.0,
    );
    println!("metal {metal:.4}, plastic {plastic:.4}");
    assert!(
        (metal - plastic).abs() > 0.05 * plastic,
        "metal {metal} plastic {plastic}"
    );
}

#[test]
fn draws_without_a_material_draw_the_same_bytes_beside_materials() {
    let Some(gpu) = gpu() else {
        return;
    };
    let plain = |with_material: bool| {
        let mut s = uniform_sky_scene();
        s.sun.diffuse = [0.7; 3];
        s.sun.towards = [0.3, -0.5, 0.8];
        s.objects.push(object(
            1,
            [-2.0, 0.0, 0.0],
            RADIUS,
            sphere([-2.0, 0.0, 0.0], RADIUS),
            None,
        ));
        // A draw out of the view behind the eye: of a material (the table holds one) or not.
        let id = with_material.then(|| {
            s.materials
                .intern(HdMaterial::of_factors(&PbrMaterial::default()))
        });
        let at = [0.0, -200.0, 0.0];
        s.objects
            .push(object(2, at, RADIUS, sphere(at, RADIUS), id));
        let cam = camera(&mut s, [0.0, -10.0, 1.0], [0.0; 3], 40);
        let mut world: HdWorld = HdWorld::new(&gpu);
        world.upload(&s);
        let target = gpu.offscreen(SIZE.0, SIZE.1).expect("a target");
        let format = target.color.texture().format();
        for _ in 0..3 {
            world
                .draw(&s, &cam, Some((&target.color, format, [0, 0])))
                .expect("a frame");
        }
        gpu.read_back(&target).expect("read back").rgba
    };
    let without = plain(false);
    let beside = plain(true);
    let differing = without
        .as_chunks::<4>()
        .0
        .iter()
        .zip(beside.as_chunks::<4>().0)
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(differing, 0, "of {} pixels", without.len() / 4);
}

#[test]
fn textures_past_the_devices_side_or_with_too_many_levels_upload_without_an_error() {
    let Some(gpu) = gpu() else {
        return;
    };
    let max = HdWorld::<torque_render_hd::ext::NoExtension>::new(&gpu)
        .device()
        .limits()
        .max_texture_dimension_2d;
    let mut s = uniform_sky_scene();
    // Twice the device's side, one row: its first level does not fit.
    let wide = 2 * max;
    let levels: Vec<Vec<u8>> = (0..=wide.ilog2())
        .map(|k| vec![200u8; 4 * (wide >> k).max(1) as usize])
        .collect();
    let a = s.textures.intern(torque_render_scene::Texture {
        format: torque_render_scene::TexelFormat::Rgba8,
        path: "wide".into(),
        width: wide,
        height: 1,
        levels: std::sync::Arc::new(levels),
    });
    // 4 x 4 with twenty levels.
    let b = s.textures.intern(torque_render_scene::Texture {
        format: torque_render_scene::TexelFormat::Rgba8,
        path: "deep".into(),
        width: 4,
        height: 4,
        levels: std::sync::Arc::new(vec![vec![90u8; 64]; 20]),
    });
    let sampler = torque_render_scene::Sampler {
        repeat: true,
        mag: torque_render_scene::Filter::Linear,
        min: torque_render_scene::Filter::Linear,
        mip: torque_render_scene::Filter::Linear,
        mipmaps: true,
    };
    for (k, t) in [a, b].into_iter().enumerate() {
        let c = [k as f32 * 3.0 - 1.5, 0.0, 0.0];
        let mut o = object(k as u32 + 1, c, RADIUS, sphere(c, RADIUS), None);
        o.draws[0].texture = Some((t, sampler));
        for v in &mut o.draws[0].vertices {
            v.material |= material::tex_env(torque_render_scene::TexEnv::Modulate);
        }
        s.objects.push(o);
    }
    let cam = camera(&mut s, [0.0, -10.0, 1.0], [0.0; 3], 40);
    let world = draw_frames(&gpu, &s, &cam, 2);
    world.finish().expect("no device error");
}

/// A 64 x 64 material's maps: a colored grain, normals leaning toward and away from the sun in bands, and an ORM of two
/// roughnesses.
fn banded_maps() -> MapImages {
    let enc = |v: f32| ((v * 0.5 + 0.5) * 255.0).round() as u8;
    MapImages {
        base: Some(map(64, 64, |x, y| {
            [150 + (x % 7 * 9) as u8, 120 + (y % 5 * 11) as u8, 90, 255]
        })),
        normal: Some(map(64, 64, |x, _| {
            let a = ((x as f32 / 64.0) * std::f32::consts::TAU * 2.0).sin() * 0.5;
            [enc(a.sin()), 128, enc(a.cos()), 255]
        })),
        orm: Some(map(64, 64, |_, y| {
            [255, if y < 32 { 90 } else { 200 }, 0, 255]
        })),
        ..MapImages::default()
    }
}

#[test]
fn a_baked_material_draws_as_its_uncompressed_maps() {
    let Some(gpu) = gpu() else {
        return;
    };
    let m = PbrMaterial::default();
    let plain = lit_floor_of(&gpu, |s| {
        pbr::build(&m, "floor", banded_maps(), &mut s.textures).expect("the maps")
    });
    let files: std::collections::HashMap<String, Vec<u8>> = pbr::bake(&m, banded_maps(), None)
        .expect("bakes")
        .into_iter()
        .map(|b| (format!("{}.ktx2", b.role.name()), b.file))
        .collect();
    let named = PbrMaterial {
        base: Some("base.ktx2".into()),
        normal: Some("normal.ktx2".into()),
        orm: Some("orm.ktx2".into()),
        ..m.clone()
    };
    let mut formats = Vec::new();
    let baked = lit_floor_of(&gpu, |s| {
        let mut read = |p: &str| files.get(p).cloned();
        let built = pbr::load(&named, "floor", &mut read, &mut s.textures).expect("loads");
        formats = s.textures.textures.iter().map(|t| t.format).collect();
        built
    });
    println!("uncompressed {plain:.4}, baked {baked:.4}");
    assert!(
        formats
            .iter()
            .all(|f| matches!(f, torque_render_scene::TexelFormat::Block(_)))
    );
    assert!((baked - plain).abs() < 0.02 * plain, "{baked} vs {plain}");
}

#[test]
fn a_block_texture_of_partial_blocks_is_decoded_on_the_cpu() {
    let Some(gpu) = gpu() else {
        return;
    };
    // 6 x 6: not whole blocks, so no GPU takes it as BC7; it goes up decoded.
    let color = [180u8, 140, 100, 255];
    let rgba = |w: u32, h: u32| color.repeat((w * h) as usize);
    let levels: Vec<Vec<u8>> = [(6, 6), (3, 3), (1, 1)]
        .into_iter()
        .map(|(w, h)| images::bc::encode(images::bc::BlockFormat::Bc7, &rgba(w, h), w, h).unwrap())
        .collect();
    let m = PbrMaterial::default();
    let blocks = lit_floor_of(&gpu, |s| {
        let base = s.textures.intern(torque_render_scene::Texture {
            path: "blocks".into(),
            width: 6,
            height: 6,
            levels: std::sync::Arc::new(levels),
            format: torque_render_scene::TexelFormat::Block(images::bc::BlockFormat::Bc7),
        });
        HdMaterial {
            base: Some(base),
            ..HdMaterial::of_factors(&m)
        }
    });
    let plain = lit_floor(
        &gpu,
        MapImages {
            base: Some(map(6, 6, |_, _| color)),
            ..MapImages::default()
        },
        m.roughness,
        m.metal,
    );
    println!("decoded {blocks:.4}, RGBA8 {plain:.4}");
    assert!((blocks - plain).abs() < 0.02 * plain, "{blocks} vs {plain}");
}
