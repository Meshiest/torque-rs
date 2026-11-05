//! Pictures of the day's light and a spot light, drawn offscreen: a checkered ground, a few boxes and a box car with a spot light
//! at its front, at dawn, noon, golden hour, sunset, dusk and night (the sun of a [`TimeOfDay`], its light the physical sky's,
//! [`ScatterTable::light`], with volumetric clouds), and the car's light off and on at dusk and at night. Each picture is a PNG in the directory given.
//!
//! `cargo run -p torque-render-hd --release --example lights-sky -- <dir>` (default `lights-sky`)

use std::path::PathBuf;
use std::sync::Arc;

use torque_render_core::Gpu;
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::hd::scatter::{ScatterLook, ScatterTable};
use torque_render_scene::hd::{
    DynamicObject, HdCamera, HdDraw, HdLight, HdMode, HdScene, HdSpot, Quality, material,
};
use torque_render_scene::{Blend, Cull, Draw, Filter, Sampler, TexEnv, Texture, Vertex};
use torque_scene::time_of_day::{DayFields, TimeOfDay};

const SIZE: (u32, u32) = (960, 540);
/// Frames drawn for each picture (the anti-aliasing's history settles over them).
const FRAMES: usize = 24;
const FOV: u8 = 70;
/// The car's object id (its spot light's carrier).
const CAR: u32 = 1;

/// One picture: its name, the time of day and whether the car's light is on.
struct Picture {
    name: &'static str,
    time: f32,
    spot: bool,
}

const PICTURES: [Picture; 8] = [
    Picture {
        name: "1-dawn",
        time: 0.012,
        spot: false,
    },
    Picture {
        name: "2-noon",
        time: 0.22,
        spot: false,
    },
    Picture {
        name: "3-golden",
        time: 0.465,
        spot: false,
    },
    Picture {
        name: "4-sunset",
        time: 0.4945,
        spot: false,
    },
    Picture {
        name: "5-dusk",
        time: 0.512,
        spot: false,
    },
    Picture {
        name: "6-dusk-light",
        time: 0.512,
        spot: true,
    },
    Picture {
        name: "7-night",
        time: 0.7,
        spot: false,
    },
    Picture {
        name: "8-night-light",
        time: 0.7,
        spot: true,
    },
];

/// The look of the pictures' sky (the night's colors and the moon's).
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

/// A checker of two grays, 64 texels a side in 8 squares.
fn checker() -> Texture {
    let mut rgba = Vec::with_capacity(64 * 64 * 4);
    for y in 0..64 {
        for x in 0..64 {
            let v = if (x / 32 + y / 32) % 2 == 0 { 200 } else { 120 };
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
    }
    Texture {
        path: "checker".into(),
        width: 64,
        height: 64,
        levels: Arc::new(vec![rgba]),
    }
}

/// The ground: `half` units each way at height 0, the checker every 4 units.
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

/// A box in `color`.
fn tinted(id: u32, center: [f32; 3], half: [f32; 3], color: [u8; 3]) -> DynamicObject {
    let mut o = box_object(id, center, half);
    for d in &mut o.draws {
        for v in &mut d.vertices {
            v.color = [color[0], color[1], color[2], 255];
        }
    }
    o
}

/// The scene's boxes: the car (a body and a cabin, as one object) and a few blocks about it.
fn objects() -> Vec<DynamicObject> {
    let mut car = tinted(CAR, [0.0, 0.0, 0.55], [1.0, 2.0, 0.45], [170, 40, 35]);
    let cabin = tinted(CAR, [0.0, -0.4, 1.3], [0.8, 1.0, 0.35], [60, 70, 90]);
    let base = car.draws[0].vertices.len() as u32;
    let d = &mut car.draws[0];
    d.vertices.extend_from_slice(&cabin.draws[0].vertices);
    d.indices
        .extend(cabin.draws[0].indices.iter().map(|i| i + base));
    car.radius = 2.6;
    vec![
        car,
        tinted(2, [-4.0, 9.0, 1.0], [1.0, 1.0, 1.0], [210, 200, 180]),
        tinted(3, [3.5, 12.0, 2.0], [1.0, 1.0, 2.0], [90, 140, 200]),
        tinted(4, [-1.0, 16.0, 0.5], [3.0, 0.5, 0.5], [120, 180, 90]),
        tinted(5, [7.0, 4.0, 3.0], [0.6, 0.6, 3.0], [220, 220, 220]),
        tinted(6, [-7.0, 2.0, 0.75], [0.75, 0.75, 0.75], [200, 150, 60]),
    ]
}

/// The car's light: at its front, pointing ahead and a little down.
fn headlight() -> HdSpot {
    let range = 40.0f32;
    // Brightness 1 at half the range (the falloff's inverse square with its 2-unit core undone there).
    let gain = 1.0 + 0.25 * (range / 2.0).powi(2);
    HdSpot {
        light: HdLight {
            position: [0.0, 2.1, 0.8],
            color: [1.0, 0.85, 0.65].map(|c: f32| c * gain),
            radius: range,
            carrier: Some(CAR),
        },
        direction: [0.0, 1.0, -0.12],
        inner: 10f32.to_radians(),
        outer: 26f32.to_radians(),
        shadows: true,
    }
}

fn main() -> Result<(), String> {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "lights-sky".into()),
    );
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let mut world: HdWorld<NoExtension> = HdWorld::new(&gpu);
    let mut scene = PlainScene {
        frame: HdScene::new(StaticCells::default()),
        mode: Some(HdMode::new(Quality::High)),
        time: 0.0,
    };
    scene.frame.atmosphere = true;
    let texture = scene.frame.textures.intern(checker());
    scene.frame.planes = vec![HdDraw::from_fixed(
        &ground(300.0),
        material::KIND_PLANE,
        &[texture],
    )];
    scene.frame.objects = objects();
    scene.frame.visible_distance = 600.0;
    scene.frame.fog_distance = 200.0;
    scene.frame.fog.start = 200.0;
    scene.frame.fog.end = 600.0;
    let table = ScatterTable::earth();
    let look = look();
    let eye = look_at([-9.0, -10.0, 4.5], [0.0, 6.0, 0.5]);
    let target = gpu.offscreen(SIZE.0, SIZE.1).map_err(|e| e.to_string())?;
    let camera = HdCamera::new(eye, FOV, scene.frame.visible_distance, SIZE.0, SIZE.1);
    for p in &PICTURES {
        // The sun rises behind the camera and sets ahead of it, beyond the boxes.
        let mut day = TimeOfDay::new(DayFields {
            azimuth_override: 200f32.to_radians(),
            play: false,
            ..DayFields::default()
        });
        day.set_time(p.time);
        let sun = day.towards();
        let light = table.light(sun, &look);
        let s = &mut scene.frame;
        s.sun = light.sun();
        s.fog = light.fog(s.fog);
        s.clear = light.fog;
        s.atmosphere_sun = Some(sun);
        s.clouds = Some(light.clouds([1.0, 0.4, 0.0]));
        s.spots = if p.spot {
            vec![headlight()]
        } else {
            Vec::new()
        };
        s.lights_generation += 1;
        s.eye = Some(eye);
        s.fov = FOV;
        s.size = SIZE;
        world.reset_history();
        let mut frame = None;
        for _ in 0..FRAMES {
            scene.frame.generation += 1;
            scene.time += 1.0 / 30.0;
            world.upload(&scene);
            world
                .draw(
                    &scene,
                    &camera,
                    Some((&target.color, target.color.texture().format(), [0, 0])),
                )
                .map_err(|e| format!("{}: {e}", p.name))?;
            frame = Some(gpu.read_back(&target).map_err(|e| e.to_string())?);
        }
        let frame = frame.ok_or("no frame")?;
        let path = dir.join(format!("{}.png", p.name));
        let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), frame.width, frame.height);
        enc.set_color(png::ColorType::Rgb);
        enc.write_header()
            .and_then(|mut w| w.write_image_data(&frame.rgb()))
            .map_err(|e| e.to_string())?;
        println!(
            "{}: sun {:.1} deg, light {:?}, ambient {:?}",
            p.name,
            sun[2].asin().to_degrees(),
            light.diffuse,
            light.ambient
        );
    }
    Ok(())
}
