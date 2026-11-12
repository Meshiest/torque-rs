//! Pictures of a room's diffuse ambient, drawn offscreen: a white room with one red wall and a white block in it, open to the
//! afternoon sun and the physical sky on one side (the sun reaching the red wall), seen from the room's middle with the red wall behind, drawn with the fixed-function ambient (the scene's ambient shaped by the sky,
//! `ImageSettings::sky_irradiance` off), with the sky's irradiance at Medium (the sky cube's alone) and at High (the light
//! capture's bounce: the red wall tints what faces it, the room is lit by what it sees). Each picture is a PNG in the directory
//! given.
//!
//! `cargo run -p torque-render-hd --release --example irradiance-room -- <dir>` (default `irradiance-room`)

use std::path::PathBuf;

use torque_render_core::Gpu;
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::hd::scatter::{ScatterLook, ScatterTable};
use torque_render_scene::hd::{
    DynamicObject, HdCamera, HdDraw, HdMode, HdScene, Quality, material,
};
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Vertex};

const SIZE: (u32, u32) = (960, 540);
/// Frames drawn for each picture (the anti-aliasing's history and the capture's faces settle over them).
const FRAMES: usize = 24;
const FOV: u8 = 75;

/// One picture: its name, the quality and whether the ambient is the sky's irradiance.
struct Picture {
    name: &'static str,
    quality: Quality,
    irradiance: bool,
}

const PICTURES: [Picture; 3] = [
    Picture {
        name: "1-fixed-function",
        quality: Quality::High,
        irradiance: false,
    },
    Picture {
        name: "2-sky-medium",
        quality: Quality::Medium,
        irradiance: true,
    },
    Picture {
        name: "3-bounce-high",
        quality: Quality::High,
        irradiance: true,
    },
];

/// A quad of `color` through the corners `c` (in order), its normal `n`: a plane of the room.
fn quad(c: [[f32; 3]; 4], n: [f32; 3], color: [u8; 3]) -> HdDraw {
    let v = |p: [f32; 3]| Vertex {
        position: p,
        normal: n,
        color: [color[0], color[1], color[2], 255],
        uv: [0.0; 2],
    };
    let d = Draw {
        label: "room".into(),
        vertices: vec![v(c[0]), v(c[1]), v(c[2]), v(c[0]), v(c[2]), v(c[3])],
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
    };
    HdDraw::from_fixed(&d, material::KIND_PLANE, &[])
}

/// The room (12 x 10 x 4 units about the origin, open toward -y, its faces turned in), the red wall at +x, and the ground
/// outside: planes, which the light capture draws (it leaves out the shapes whose bounds hold the eye).
fn planes() -> Vec<HdDraw> {
    let white = [215, 212, 205];
    let (x, y, z) = (6.0, 5.0, 4.0);
    vec![
        quad(
            [
                [-40.0, -40.0, 0.0],
                [40.0, -40.0, 0.0],
                [40.0, 40.0, 0.0],
                [-40.0, 40.0, 0.0],
            ],
            [0.0, 0.0, 1.0],
            [150, 145, 135],
        ),
        quad(
            [[-x, -y, z], [x, -y, z], [x, y, z], [-x, y, z]],
            [0.0, 0.0, -1.0],
            white,
        ),
        quad(
            [[x, -y, 0.0], [x, y, 0.0], [x, y, z], [x, -y, z]],
            [-1.0, 0.0, 0.0],
            [190, 30, 25],
        ),
        quad(
            [[-x, -y, 0.0], [-x, y, 0.0], [-x, y, z], [-x, -y, z]],
            [1.0, 0.0, 0.0],
            white,
        ),
        quad(
            [[-x, y, 0.0], [x, y, 0.0], [x, y, z], [-x, y, z]],
            [0.0, -1.0, 0.0],
            white,
        ),
        // The lintel over the open side.
        quad(
            [[-x, -y, 3.0], [x, -y, 3.0], [x, -y, z], [-x, -y, z]],
            [0.0, 1.0, 0.0],
            white,
        ),
    ]
}

/// The white block in the room.
fn block() -> DynamicObject {
    let mut o = box_object(1, [-2.8, 1.2, 0.9], [0.9, 0.9, 0.9]);
    for d in &mut o.draws {
        for v in &mut d.vertices {
            v.color = [235, 235, 235, 255];
        }
    }
    o
}

fn main() -> Result<(), String> {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "irradiance-room".into()),
    );
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let mut world: HdWorld<NoExtension> = HdWorld::new(&gpu);
    let mut scene = PlainScene {
        frame: HdScene::new(StaticCells::default()),
        mode: None,
        time: 0.0,
    };
    let s = &mut scene.frame;
    s.atmosphere = true;
    s.planes = planes();
    s.objects = vec![block()];
    s.visible_distance = 600.0;
    s.fog_distance = 300.0;
    s.fog.start = 300.0;
    s.fog.end = 600.0;
    // A sun in front of the open side and to its left: its light reaches across the floor onto the red wall.
    let sun = {
        let (az, el) = (230f32.to_radians(), 30f32.to_radians());
        [az.sin() * el.cos(), az.cos() * el.cos(), el.sin()]
    };
    let light = ScatterTable::earth().light(sun, &ScatterLook::default());
    s.sun = light.sun();
    s.fog = light.fog(s.fog);
    s.clear = light.fog;
    s.atmosphere_sun = Some(sun);
    // From the middle of the room toward the white wall across from the red one: that wall and the block's near face face the
    // red wall behind the camera.
    let eye = look_at([0.5, -3.6, 2.2], [-5.0, 2.5, 0.8]);
    s.eye = Some(eye);
    s.fov = FOV;
    s.size = SIZE;
    let target = gpu.offscreen(SIZE.0, SIZE.1).map_err(|e| e.to_string())?;
    let camera = HdCamera::new(
        eye,
        f32::from(FOV),
        scene.frame.visible_distance,
        SIZE.0,
        SIZE.1,
    );
    for p in &PICTURES {
        let mut mode = HdMode::new(p.quality);
        mode.image.sky_irradiance = p.irradiance;
        scene.mode = Some(mode);
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
        println!("{}", path.display());
    }
    Ok(())
}
