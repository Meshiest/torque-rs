//! Pictures of the volumetric light at Ultra, drawn offscreen, each scene once with the volumetric march alone and once with the
//! froxel grid beside it (`HdFeatures::froxel_fog`): a foggy dusk with a box car's two headlights and a street lamp, a low sun's
//! shafts through a grove of box trees, and a room in haze lit by a lamp and the sun through a window. Writes `<scene>-march.png`,
//! `<scene>-froxels.png` and `sheet.png` (a row a scene, the march on the left) into the directory given; with `--showcase`
//! only froxel pictures, of the foggy dusk, a night street of lamps in fog and the room (no sheet).
//!
//! `cargo run -p torque-render-hd --release --example fog-lights -- [<dir>] [--size WxH] [--showcase]` (default
//! `fog-lights`, 960 x 540)

use std::path::{Path, PathBuf};
use std::sync::Arc;

use torque_render_core::Gpu;
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::hd::scatter::{ScatterLook, ScatterTable};
use torque_render_scene::hd::sky::Volumetric;
use torque_render_scene::hd::{
    DynamicObject, HdCamera, HdDraw, HdLight, HdMode, HdScene, HdSpot, ImageSettings, Quality,
    material,
};
use torque_render_scene::{Blend, Cull, Draw, Filter, Sampler, TexEnv, Texture, Vertex};
use torque_scene::time_of_day::{DayFields, TimeOfDay};

/// Frames drawn for each picture (the histories settle over them).
const FRAMES: usize = 48;
const FOV: u8 = 70;
const CAR: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Look {
    DuskFog,
    SunShafts,
    IndoorHaze,
    NightStreet,
}

const SHOWCASE: [(Look, &str); 3] = [
    (Look::DuskFog, "dusk-fog"),
    (Look::NightStreet, "night-street"),
    (Look::IndoorHaze, "indoor-haze"),
];

const LOOKS: [(Look, &str); 3] = [
    (Look::DuskFog, "dusk-fog"),
    (Look::SunShafts, "sun-shafts"),
    (Look::IndoorHaze, "indoor-haze"),
];

fn look() -> ScatterLook {
    ScatterLook {
        shadow: [0.12, 0.13, 0.16],
        night_color: [0.11, 0.13, 0.19],
        night_fog_color: [0.04, 0.05, 0.08],
        moon_azimuth: 200.0,
        moon_elevation: 50.0,
        moon_light_color: [0.05, 0.06, 0.09],
        ..ScatterLook::default()
    }
}

/// A checker of two grays, 64 texels a side in 4 squares.
fn checker() -> Texture {
    let mut rgba = Vec::with_capacity(64 * 64 * 4);
    for y in 0..64 {
        for x in 0..64 {
            let v = if (x / 32 + y / 32) % 2 == 0 { 150 } else { 110 };
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
    }
    Texture {
        format: torque_render_scene::TexelFormat::Rgba8,
        path: "checker".into(),
        width: 64,
        height: 64,
        levels: Arc::new(vec![rgba]),
    }
}

/// The ground: `half` units each way at height 0.
fn ground(half: f32) -> Draw {
    let corner = |x: f32, y: f32| Vertex {
        position: [x, y, 0.0],
        normal: [0.0, 0.0, 1.0],
        color: [255; 4],
        uv: [x / 8.0, y / 8.0],
    };
    let (a, b, c, d) = (
        corner(-half, -half),
        corner(half, -half),
        corner(half, half),
        corner(-half, half),
    );
    Draw {
        label: "ground".into(),
        vertices: vec![a, b, c, a, c, d],
        lighting: true,
        tex_env: TexEnv::Modulate,
        texture: Some((
            0,
            Sampler {
                repeat: true,
                mag: Filter::Linear,
                min: Filter::Linear,
                mip: Filter::Linear,
                mipmaps: true,
            },
        )),
        blend: Blend::Opaque,
        cull: Cull::None,
        depth_test: true,
        depth_write: true,
        fog: true,
        lights: Vec::new(),
        vertex_key: None,
    }
}

fn tinted(id: u32, center: [f32; 3], half: [f32; 3], color: [u8; 3]) -> DynamicObject {
    let mut o = box_object(id, center, half);
    for d in &mut o.draws {
        for v in &mut d.vertices {
            v.color = [color[0], color[1], color[2], 255];
        }
    }
    o
}

/// Boxes as one object (`id`): each a center, half sizes and a color.
fn joined(id: u32, parts: &[([f32; 3], [f32; 3], [u8; 3])]) -> DynamicObject {
    let mut out = tinted(id, parts[0].0, parts[0].1, parts[0].2);
    for &(c, h, col) in &parts[1..] {
        let o = tinted(id, c, h, col);
        let base = out.draws[0].vertices.len() as u32;
        let d = &mut out.draws[0];
        d.vertices.extend_from_slice(&o.draws[0].vertices);
        d.indices
            .extend(o.draws[0].indices.iter().map(|i| i + base));
    }
    let r = parts
        .iter()
        .map(|(c, h, _)| (c[0].hypot(c[1]).hypot(c[2])) + h[0].hypot(h[1]).hypot(h[2]))
        .fold(0.0f32, f32::max);
    out.radius = r;
    out
}

/// A headlight at `x` across the car's front, pointing ahead and a little down.
fn headlight(x: f32) -> HdSpot {
    let range = 36.0f32;
    let gain = 1.0 + 0.25 * (range / 2.0).powi(2);
    HdSpot {
        light: HdLight {
            position: [x, 2.05, 0.75],
            color: [1.0, 0.88, 0.7].map(|c: f32| c * gain * 0.5),
            radius: range,
            carrier: Some(CAR),
        },
        direction: [0.0, 1.0, -0.1],
        inner: 9f32.to_radians(),
        outer: 22f32.to_radians(),
        shadows: true,
    }
}

/// A warm point light at `at` reaching `radius`, `brightness` at half the radius.
fn lamp(at: [f32; 3], radius: f32, brightness: f32) -> HdLight {
    let gain = brightness * (1.0 + 0.25 * (radius / 2.0).powi(2));
    HdLight {
        position: at,
        color: [1.0, 0.72, 0.42].map(|c: f32| c * gain),
        radius,
        carrier: None,
    }
}

/// The sun at `t` of a turn of the day with its azimuth held at `azimuth` degrees (past a quarter of the turn it stands over the
/// opposite heading), lighting `s`.
fn set_sun(s: &mut HdScene<StaticCells>, t: f32, azimuth: f32) {
    let mut day = TimeOfDay::new(DayFields {
        azimuth_override: azimuth.to_radians(),
        play: false,
        ..DayFields::default()
    });
    day.set_time(t);
    let sun = day.towards();
    let light = ScatterTable::earth().light(sun, &look());
    s.sun = light.sun();
    s.fog = light.fog(s.fog);
    s.clear = light.fog;
    s.atmosphere_sun = Some(sun);
}

/// The scene of `look` and its eye.
fn scene(which: Look) -> (PlainScene, [[f32; 3]; 2]) {
    let mut s = HdScene::new(StaticCells::default());
    s.atmosphere = true;
    s.sun_radius = 0.27;
    let texture = s.textures.intern(checker());
    s.planes = vec![HdDraw::from_fixed(
        &ground(300.0),
        material::KIND_PLANE,
        &[texture],
    )];
    let eye = match which {
        Look::DuskFog => {
            s.visible_distance = 260.0;
            (s.fog.start, s.fog.end) = (2.0, 80.0);
            set_sun(&mut s, 0.506, 200.0);
            let car = joined(
                CAR,
                &[
                    ([0.0, 0.0, 0.55], [1.0, 2.0, 0.45], [150, 40, 35]),
                    ([0.0, -0.4, 1.3], [0.8, 1.0, 0.35], [50, 60, 80]),
                ],
            );
            let pole = joined(
                2,
                &[
                    ([6.0, 14.0, 3.0], [0.12, 0.12, 3.0], [70, 70, 75]),
                    ([5.4, 14.0, 6.0], [0.7, 0.18, 0.12], [70, 70, 75]),
                ],
            );
            s.objects = vec![
                car,
                pole,
                tinted(3, [-6.0, 18.0, 1.5], [2.0, 2.0, 1.5], [120, 110, 100]),
                tinted(4, [9.0, 28.0, 3.0], [3.0, 2.0, 3.0], [110, 100, 95]),
                tinted(5, [-3.0, 34.0, 2.0], [1.5, 1.5, 2.0], [100, 105, 110]),
            ];
            s.spots = vec![headlight(-0.6), headlight(0.6)];
            s.lights = vec![lamp([4.9, 14.0, 5.75], 16.0, 0.6)];
            [[-8.0, -1.0, 1.9], [1.5, 12.0, 1.6]]
        }
        Look::SunShafts => {
            s.visible_distance = 400.0;
            (s.fog.start, s.fog.end) = (20.0, 600.0);
            set_sun(&mut s, 0.47, 180.0);
            let mut trees = Vec::new();
            let mut id = 10;
            for gy in 0..6 {
                for gx in 0..7 {
                    let jitter = ((gx * 7 + gy * 13) % 5) as f32 * 0.6;
                    let x = -18.0 + gx as f32 * 6.0 + jitter;
                    let y = 8.0 + gy as f32 * 7.0 + jitter * 0.5;
                    let h = 5.0 + ((gx + gy) % 3) as f32;
                    trees.push(joined(
                        id,
                        &[
                            ([x, y, h * 0.5], [0.25, 0.25, h * 0.5], [80, 60, 40]),
                            ([x, y, h + 1.2], [1.8, 1.8, 1.4], [50, 90, 45]),
                            ([x + 0.6, y - 0.4, h + 2.6], [1.1, 1.1, 0.8], [55, 95, 50]),
                        ],
                    ));
                    id += 1;
                }
            }
            s.objects = trees;
            [[0.0, -6.0, 2.0], [4.0, 30.0, 4.5]]
        }
        Look::NightStreet => {
            s.visible_distance = 260.0;
            (s.fog.start, s.fog.end) = (0.0, 70.0);
            set_sun(&mut s, 0.56, 200.0);
            // A road of lamps on both sides, the car coming down it with its headlights on.
            let mut objects = Vec::new();
            let mut lights = Vec::new();
            for k in 0..6 {
                let y = 6.0 + 11.0 * k as f32;
                for side in [-1.0f32, 1.0] {
                    let x = 6.5 * side;
                    let id = 40 + 2 * k as u32 + u32::from(side > 0.0);
                    objects.push(joined(
                        id,
                        &[
                            ([x, y, 3.0], [0.1, 0.1, 3.0], [60, 60, 65]),
                            ([x - 0.6 * side, y, 6.0], [0.7, 0.15, 0.1], [60, 60, 65]),
                        ],
                    ));
                    lights.push(lamp([x - 1.0 * side, y, 5.8], 13.0, 0.5));
                }
            }
            let car = joined(
                CAR,
                &[
                    ([0.0, 0.0, 0.55], [1.0, 2.0, 0.45], [40, 60, 120]),
                    ([0.0, -0.4, 1.3], [0.8, 1.0, 0.35], [40, 45, 55]),
                ],
            );
            objects.push(car);
            s.objects = objects;
            s.lights = lights;
            s.spots = vec![headlight(-0.6), headlight(0.6)];
            [[3.5, -7.5, 1.6], [-1.0, 30.0, 3.0]]
        }
        Look::IndoorHaze => {
            s.visible_distance = 300.0;
            (s.fog.start, s.fog.end) = (0.0, 260.0);
            set_sun(&mut s, 0.46, 90.0);
            // A room 12 x 12 x 5 with a window in its west wall (x = -6).
            let gray = [170, 165, 155];
            let w = 0.3;
            s.objects = vec![
                tinted(20, [6.0 + w, 0.0, 2.5], [w, 6.0 + w, 2.5], gray),
                tinted(21, [0.0, 6.0 + w, 2.5], [6.0, w, 2.5], gray),
                tinted(22, [0.0, -6.0 - w, 2.5], [6.0, w, 2.5], gray),
                tinted(
                    23,
                    [0.0, 0.0, 5.0 + w],
                    [6.0 + 2.0 * w, 6.0 + 2.0 * w, w],
                    gray,
                ),
                // The west wall about a window 3 wide and 2 high.
                tinted(24, [-6.0 - w, -3.75, 2.5], [w, 2.25, 2.5], gray),
                tinted(25, [-6.0 - w, 3.75, 2.5], [w, 2.25, 2.5], gray),
                tinted(26, [-6.0 - w, 0.0, 0.75], [w, 1.5, 0.75], gray),
                tinted(27, [-6.0 - w, 0.0, 4.25], [w, 1.5, 0.75], gray),
                tinted(28, [1.5, 2.0, 0.4], [1.5, 0.8, 0.4], [120, 80, 50]),
                tinted(29, [3.5, -3.0, 0.5], [0.5, 0.5, 0.5], [90, 110, 140]),
            ];
            s.lights = vec![lamp([-2.5, 4.5, 3.6], 10.0, 0.45)];
            [[4.5, -5.0, 2.2], [-6.0, 1.0, 1.6]]
        }
    };
    s.fog_distance = s.fog.start;
    // The grove's and the room's air holds more than the default thin medium (dust, haze): the god rays show.
    let scattering = match which {
        Look::DuskFog | Look::NightStreet => ImageSettings::DEFAULT.volumetric.scattering,
        Look::SunShafts => 0.006,
        Look::IndoorHaze => 0.012,
    };
    let image = ImageSettings {
        volumetric: Volumetric {
            scattering,
            ..ImageSettings::DEFAULT.volumetric
        },
        ..ImageSettings::DEFAULT
    };
    s.lights_generation += 1;
    (
        PlainScene {
            frame: s,
            mode: Some(HdMode {
                image,
                quality: Quality::Ultra,
            }),
            time: 0.0,
        },
        eye,
    )
}

/// The scene drawn at `size` with the froxel grid or the march, as RGB bytes.
fn picture(gpu: &Gpu, which: Look, size: (u32, u32), froxels: bool) -> Result<Vec<u8>, String> {
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    world.features.froxel_fog = froxels;
    let (mut scene, [at, target]) = scene(which);
    let eye = look_at(at, target);
    let s = &mut scene.frame;
    s.eye = Some(eye);
    s.fov = FOV;
    s.size = size;
    let camera = HdCamera::new(eye, FOV, s.visible_distance, size.0, size.1);
    let out = gpu.offscreen(size.0, size.1).map_err(|e| e.to_string())?;
    for _ in 0..FRAMES {
        scene.frame.generation += 1;
        scene.time += 1.0 / 60.0;
        world.upload(&scene);
        world.draw(
            &scene,
            &camera,
            Some((&out.color, out.color.texture().format(), [0, 0])),
        )?;
    }
    Ok(gpu.read_back(&out).map_err(|e| e.to_string())?.rgb())
}

fn write_png(path: &Path, (w, h): (u32, u32), rgb: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(rgb))
        .map_err(|e| e.to_string())
}

fn main() -> Result<(), String> {
    let mut dir = PathBuf::from("fog-lights");
    let mut size = (960u32, 540u32);
    let mut showcase = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--size" => {
                let v = args.next().ok_or("--size wants WxH")?;
                let (w, h) = v.split_once('x').ok_or("--size wants WxH")?;
                size = (
                    w.parse().map_err(|e| format!("{e}"))?,
                    h.parse().map_err(|e| format!("{e}"))?,
                );
            }
            "--showcase" => showcase = true,
            _ => dir = PathBuf::from(a),
        }
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    if showcase {
        for (which, name) in SHOWCASE {
            let after = picture(&gpu, which, size, true)?;
            write_png(&dir.join(format!("{name}.png")), size, &after)?;
        }
        return Ok(());
    }
    let mut sheet = Vec::new();
    for (which, name) in LOOKS {
        let after = picture(&gpu, which, size, true)?;
        write_png(&dir.join(format!("{name}-froxels.png")), size, &after)?;
        let before = picture(&gpu, which, size, false)?;
        write_png(&dir.join(format!("{name}-march.png")), size, &before)?;
        let row = size.0 as usize * 3;
        for y in 0..size.1 as usize {
            sheet.extend_from_slice(&before[y * row..(y + 1) * row]);
            sheet.extend_from_slice(&after[y * row..(y + 1) * row]);
        }
        println!("{name}");
    }
    write_png(
        &dir.join("sheet.png"),
        (size.0 * 2, size.1 * LOOKS.len() as u32),
        &sheet,
    )
}
