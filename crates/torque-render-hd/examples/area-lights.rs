//! Pictures of a room lit by area lights, drawn offscreen at 1920 x 1080 (Ultra): a closed room at night with a gloss floor, a
//! blue wall and three blocks, lit by a ceiling panel (a rectangle light, `1-ceiling-panel.png`) and by a fluorescent tube on the
//! ceiling (a tube light, `2-ceiling-tube.png`), each casting its shadows from its center. The lights' own shapes are unlit
//! draws of their color. Each picture is a PNG in the directory given.
//!
//! `cargo run -p torque-render-hd --release --example area-lights -- <dir>` (default `area-lights`); with `--cost` in place of
//! the directory it prints the opaque pass's GPU time with eight lights over the room as points, rectangles and tubes, and with
//! none (under the bench lock).

use std::path::PathBuf;

use torque_render_core::{Gpu, wgpu};
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{ExtCell, HdExtension, PlainScene, Surfaces};
use torque_render_hd::profile::Profile;
use torque_render_hd::shaders::{NONE, ShaderExtension};
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::hd::{
    AreaShape, DynamicObject, HdAreaLight, HdCamera, HdDraw, HdLight, HdMode, HdScene, Quality,
    material,
};
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Vertex};

const SIZE: (u32, u32) = (1920, 1080);
/// Frames drawn for each picture (the anti-aliasing's history and the capture's faces settle over them).
const FRAMES: usize = 32;
const FOV: u8 = 90;
/// Frames timed for each light set of `--cost` (after 30 that settle).
const COST_FRAMES: usize = 60;
/// The room's half extents (x, y) and height.
const ROOM: [f32; 3] = [4.0, 3.0, 3.0];

/// The engine's shading with a glossier floor (the planes' roughness 0.3) and blocks (0.45), so the lights' highlights show.
struct Room;

impl HdExtension for Room {
    type Geometry = StaticCells;
    type Scene = PlainScene;
    type CellRuns = ();
    type LevelInput = ();
    const FRAME_EXT_BYTES: usize = 16;

    fn new(_device: &wgpu::Device) -> Room {
        Room
    }

    fn shaders() -> ShaderExtension {
        NONE
    }

    fn mode(scene: &PlainScene) -> Option<HdMode> {
        scene.mode
    }

    fn time(scene: &PlainScene) -> f32 {
        scene.time
    }

    fn level_input(_cell: &ExtCell<Room>) {}

    fn surfaces(&self, scene: &PlainScene) -> Surfaces {
        let mut s = Surfaces::of(&Self::mode(scene).unwrap_or_else(Self::fallback_mode).image);
        s.table[4] = 0.45;
        s.table[8] = 0.3;
        s
    }
}

/// A quad of `color` through the corners `c` (in order), its normal `n`; `lit` false: an emitter drawn as its color.
fn quad(c: [[f32; 3]; 4], n: [f32; 3], color: [u8; 3], lit: bool) -> HdDraw {
    let v = |p: [f32; 3]| Vertex {
        position: p,
        normal: n,
        color: [color[0], color[1], color[2], 255],
        uv: [0.0; 2],
    };
    let d = Draw {
        label: "room".into(),
        vertices: vec![v(c[0]), v(c[1]), v(c[2]), v(c[0]), v(c[2]), v(c[3])],
        lighting: lit,
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

/// The room's floor, ceiling and walls (faces turned in), the wall at +x blue.
fn room() -> Vec<HdDraw> {
    let [x, y, z] = ROOM;
    let wall = [200, 196, 188];
    vec![
        quad(
            [[-x, -y, 0.0], [x, -y, 0.0], [x, y, 0.0], [-x, y, 0.0]],
            [0.0, 0.0, 1.0],
            [120, 112, 104],
            true,
        ),
        quad(
            [[-x, -y, z], [x, -y, z], [x, y, z], [-x, y, z]],
            [0.0, 0.0, -1.0],
            wall,
            true,
        ),
        quad(
            [[x, -y, 0.0], [x, y, 0.0], [x, y, z], [x, -y, z]],
            [-1.0, 0.0, 0.0],
            [60, 90, 160],
            true,
        ),
        quad(
            [[-x, -y, 0.0], [-x, y, 0.0], [-x, y, z], [-x, -y, z]],
            [1.0, 0.0, 0.0],
            wall,
            true,
        ),
        quad(
            [[-x, y, 0.0], [x, y, 0.0], [x, y, z], [-x, y, z]],
            [0.0, -1.0, 0.0],
            wall,
            true,
        ),
        quad(
            [[-x, -y, 0.0], [x, -y, 0.0], [x, -y, z], [-x, -y, z]],
            [0.0, 1.0, 0.0],
            wall,
            true,
        ),
    ]
}

/// An emitter's box of half extents `h` about `c`, drawn as its color.
fn emitter(c: [f32; 3], h: [f32; 3], color: [u8; 3]) -> Vec<HdDraw> {
    let mut out = Vec::new();
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let corner = |a: f32, b: f32| {
                let mut p = c;
                p[axis] += sign * h[axis];
                p[u] += a * h[u];
                p[v] += b * h[v];
                p
            };
            let mut n = [0.0; 3];
            n[axis] = sign;
            out.push(quad(
                [
                    corner(-1.0, -1.0),
                    corner(1.0, -1.0),
                    corner(1.0, 1.0),
                    corner(-1.0, 1.0),
                ],
                n,
                color,
                false,
            ));
        }
    }
    out
}

/// A block of `color` (gamma) at `center` with half extents `h`.
fn block(ghost: u32, center: [f32; 3], h: [f32; 3], color: [u8; 3]) -> DynamicObject {
    let mut o = box_object(ghost, center, h);
    for d in &mut o.draws {
        for v in &mut d.vertices {
            v.color = [color[0], color[1], color[2], 255];
        }
    }
    o
}

/// One picture: its name, the area light and its shape's emitter.
struct Picture {
    name: &'static str,
    light: HdAreaLight,
    emitter: Vec<HdDraw>,
}

fn pictures() -> Vec<Picture> {
    let panel_at = [1.4, 1.0, ROOM[2] - 0.02];
    let tube_at = [1.0, 0.6, ROOM[2] - 0.12];
    vec![
        Picture {
            name: "1-ceiling-panel",
            light: HdAreaLight {
                light: HdLight {
                    position: panel_at,
                    color: [1.0, 0.92, 0.8].map(|c| c * 3.2),
                    radius: 12.0,
                    carrier: None,
                },
                // 2.4 x 1.2, lit downward.
                shape: AreaShape::Rect {
                    half_u: [0.0, 0.6, 0.0],
                    half_v: [1.2, 0.0, 0.0],
                },
                shadows: true,
            },
            emitter: emitter(panel_at, [1.2, 0.6, 0.008], [255, 246, 232]),
        },
        Picture {
            name: "2-ceiling-tube",
            light: HdAreaLight {
                light: HdLight {
                    position: tube_at,
                    color: [0.86, 0.95, 1.0].map(|c| c * 2.0),
                    radius: 12.0,
                    carrier: None,
                },
                // 1.8 long, 5 cm across.
                shape: AreaShape::Tube {
                    half_axis: [0.0, 0.9, 0.0],
                    radius: 0.025,
                },
                shadows: true,
            },
            emitter: emitter(tube_at, [0.025, 0.9, 0.025], [236, 246, 255]),
        },
    ]
}

/// The room with its blocks, at night, seen from a corner.
fn room_scene() -> (PlainScene, HdCamera) {
    let mut scene = PlainScene {
        frame: HdScene::new(StaticCells::default()),
        mode: Some(HdMode::new(Quality::Ultra)),
        time: 0.0,
    };
    let s = &mut scene.frame;
    s.objects = vec![
        block(1, [-0.6, 1.7, 0.45], [0.45, 0.45, 0.45], [235, 235, 232]),
        block(2, [2.3, -0.4, 0.3], [0.7, 0.35, 0.3], [210, 110, 50]),
        block(3, [0.6, -1.5, 0.9], [0.25, 0.25, 0.9], [150, 150, 155]),
    ];
    s.planes = room();
    s.visible_distance = 200.0;
    s.fog_distance = 150.0;
    s.fog.start = 150.0;
    s.fog.end = 200.0;
    // Night: a faint ambient, no sun.
    s.sun.ambient = [0.04, 0.04, 0.05];
    s.clear = [0.0; 3];
    // From a corner, low enough over the gloss floor to see the lights' reflections in it.
    let eye = look_at([-3.6, -2.7, 1.6], [1.0, 0.8, 0.3]);
    s.eye = Some(eye);
    s.fov = FOV;
    s.size = SIZE;
    let camera = HdCamera::new(eye, f32::from(FOV), s.visible_distance, SIZE.0, SIZE.1);
    (scene, camera)
}

/// Draws `frames` frames of the scene into `target`, the last one read back.
fn draw(
    gpu: &Gpu,
    world: &mut HdWorld<Room>,
    scene: &mut PlainScene,
    camera: &HdCamera,
    target: &torque_render_core::Offscreen,
    frames: usize,
) -> Result<torque_render_core::Frame, String> {
    let mut frame = None;
    for _ in 0..frames {
        scene.frame.generation += 1;
        scene.time += 1.0 / 30.0;
        world.upload(scene);
        world.draw(
            scene,
            camera,
            Some((&target.color, target.color.texture().format(), [0, 0])),
        )?;
        frame = Some(gpu.read_back(target).map_err(|e| e.to_string())?);
    }
    frame.ok_or_else(|| "no frame".into())
}

/// The opaque pass's GPU time (median ms) with eight lights over the room as points, rectangles and tubes (no shadows), and
/// with none: what a light the size of the frame costs.
fn cost(gpu: &Gpu) -> Result<(), String> {
    let (mut scene, camera) = room_scene();
    let mut world: HdWorld<Room> = HdWorld::new(gpu);
    let target = gpu.offscreen(SIZE.0, SIZE.1).map_err(|e| e.to_string())?;
    let at = |k: usize| {
        [
            -2.4 + 1.6 * (k % 4) as f32,
            -1.2 + 2.4 * (k / 4) as f32,
            ROOM[2] - 0.05,
        ]
    };
    let light = |k: usize| HdLight {
        position: at(k),
        color: [0.6; 3],
        radius: 14.0,
        carrier: None,
    };
    let rect = AreaShape::Rect {
        half_u: [0.0, 0.4, 0.0],
        half_v: [0.6, 0.0, 0.0],
    };
    let tube = AreaShape::Tube {
        half_axis: [0.6, 0.0, 0.0],
        radius: 0.025,
    };
    let area = |shape: AreaShape| {
        (0..8)
            .map(|k| HdAreaLight {
                light: light(k),
                shape,
                shadows: false,
            })
            .collect::<Vec<_>>()
    };
    let sets: [(&str, Vec<HdLight>, Vec<HdAreaLight>); 4] = [
        ("no lights", Vec::new(), Vec::new()),
        ("8 points", (0..8).map(light).collect(), Vec::new()),
        ("8 rectangles", Vec::new(), area(rect)),
        ("8 tubes", Vec::new(), area(tube)),
    ];
    for (name, lights, areas) in sets {
        scene.frame.lights = lights;
        scene.frame.area_lights = areas;
        scene.frame.lights_generation += 1;
        world.reset_history();
        draw(gpu, &mut world, &mut scene, &camera, &target, 30)?;
        let mut p = Profile::begin(&mut world);
        for _ in 0..COST_FRAMES {
            draw(gpu, &mut world, &mut scene, &camera, &target, 1)?;
            p.frame(&mut world);
        }
        world.profile_passes = false;
        let (rows, total) = p.gpu_medians();
        let opaque = rows.iter().find(|r| r.0 == "opaque").map_or(0.0, |r| r.1);
        println!(
            "{name}: opaque {opaque:.3} ms, frame {total:.3} ms ({}x{}, Ultra)",
            SIZE.0, SIZE.1
        );
    }
    Ok(())
}

fn main() -> Result<(), String> {
    let arg = std::env::args().nth(1);
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    if arg.as_deref() == Some("--cost") {
        return cost(&gpu);
    }
    let dir = PathBuf::from(arg.unwrap_or_else(|| "area-lights".into()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (mut scene, camera) = room_scene();
    let mut world: HdWorld<Room> = HdWorld::new(&gpu);
    let target = gpu.offscreen(SIZE.0, SIZE.1).map_err(|e| e.to_string())?;
    for p in pictures() {
        scene.frame.planes = room();
        scene.frame.planes.extend(p.emitter);
        scene.frame.area_lights = vec![p.light];
        scene.frame.lights_generation += 1;
        world.reset_history();
        let frame = draw(&gpu, &mut world, &mut scene, &camera, &target, FRAMES)
            .map_err(|e| format!("{}: {e}", p.name))?;
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
