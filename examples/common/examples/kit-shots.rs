//! Pictures of every shape of the asset kit for review, drawn offscreen on a plain grey floor: the cube; the figure from the
//! front, side and back, its face close up, and in its idle, walk, run and jump poses; the figure holding the pistol (its
//! `hold` arm pose) from the side, front three-quarters and over its shoulder; the
//! pistol from the side and three-quarters with its muzzle, eject and mount points marked; and the projectile.
//!
//! `cargo run -p torque-example-common --example kit-shots -- <dir>` (default `kit-shots`)

use std::f32::consts::{FRAC_PI_2, PI};
use std::path::PathBuf;

use torque::shapes::instance::{ShapeInstance, find_sequence};
use torque::shapes::math::mul_p;
use torque_example_common::view::{Daylight, SceneShape, WorldView};
use torque_example_common::{KitAssets, kit_path};
use torque_render_core::Gpu;
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::math::Mat;

const SIZE: (u32, u32) = (800, 600);
/// Frames drawn for each picture (the anti-aliasing's history settles over them).
const FRAMES: usize = 12;
const FOV: u8 = 50;

/// One picture: the shape, a sequence and its position (None: the rest pose), how high its origin stands, the camera's turn
/// about the shape (0: in front of it, looking along -y at its face), how high the camera stands (a share of its distance),
/// its distance (a share of the one that holds the whole shape), the point it looks at (None: the shape's centre), the nodes to
/// mark, and whether the shape holds the pistol at `mount0` (with its `hold` pose).
#[derive(Clone, Copy)]
struct Picture {
    name: &'static str,
    shape: &'static str,
    pose: Option<(&'static str, f32)>,
    lift: f32,
    turn: f32,
    rise: f32,
    zoom: f32,
    aim: Option<[f32; 3]>,
    marks: &'static [&'static str],
    hold: bool,
}

const SHAPE: Picture = Picture {
    name: "",
    shape: "figure.dts",
    pose: None,
    lift: 0.0,
    turn: 0.0,
    rise: 0.34,
    zoom: 1.0,
    aim: None,
    marks: &[],
    hold: false,
};

const PISTOL_MARKS: &[&str] = &["muzzlePoint", "ejectPoint", "mountPoint"];

const PICTURES: &[Picture] = &[
    Picture {
        name: "cube",
        shape: "cube.dts",
        lift: 0.5,
        turn: 0.6,
        ..SHAPE
    },
    Picture {
        name: "figure-front",
        ..SHAPE
    },
    Picture {
        name: "figure-side",
        turn: FRAC_PI_2,
        ..SHAPE
    },
    Picture {
        name: "figure-back",
        turn: PI,
        ..SHAPE
    },
    Picture {
        name: "figure-face",
        rise: 0.05,
        zoom: 0.22,
        aim: Some([0.0, 0.0, 1.82]),
        ..SHAPE
    },
    Picture {
        name: "figure-idle",
        pose: Some(("idle", 0.25)),
        turn: 0.7,
        ..SHAPE
    },
    Picture {
        name: "figure-walk",
        pose: Some(("walk", 0.25)),
        turn: FRAC_PI_2,
        ..SHAPE
    },
    Picture {
        name: "figure-run",
        pose: Some(("run", 0.25)),
        turn: FRAC_PI_2,
        ..SHAPE
    },
    Picture {
        name: "figure-jump",
        pose: Some(("jump", 1.0)),
        lift: 0.3,
        turn: 0.9,
        zoom: 1.25,
        ..SHAPE
    },
    Picture {
        name: "holding-side",
        pose: Some(("idle", 0.0)),
        turn: FRAC_PI_2,
        rise: 0.1,
        zoom: 0.6,
        aim: Some([0.2, 0.35, 1.3]),
        hold: true,
        ..SHAPE
    },
    Picture {
        name: "holding-front-three-quarter",
        pose: Some(("idle", 0.0)),
        turn: 0.6,
        rise: 0.15,
        zoom: 0.6,
        aim: Some([0.2, 0.35, 1.3]),
        hold: true,
        ..SHAPE
    },
    Picture {
        name: "holding-over-the-shoulder",
        pose: Some(("idle", 0.0)),
        turn: PI - 0.25,
        rise: 0.2,
        zoom: 0.55,
        aim: Some([0.33, 1.6, 1.5]),
        hold: true,
        ..SHAPE
    },
    Picture {
        name: "pistol-side",
        shape: "pistol.dts",
        lift: 0.2,
        turn: FRAC_PI_2,
        rise: 0.03,
        marks: PISTOL_MARKS,
        ..SHAPE
    },
    Picture {
        name: "pistol-three-quarter",
        shape: "pistol.dts",
        lift: 0.2,
        turn: 0.75,
        rise: 0.4,
        marks: PISTOL_MARKS,
        ..SHAPE
    },
    Picture {
        name: "projectile",
        shape: "projectile.dts",
        lift: 0.15,
        turn: 1.9,
        ..SHAPE
    },
];

/// The marks' colours, in the order of a picture's marks.
const MARK_COLOURS: [[u8; 4]; 3] = [[230, 50, 40, 255], [40, 120, 230, 255], [60, 190, 70, 255]];

/// The matrix of node `node` of `instance` (shape space).
fn node_matrix(instance: &ShapeInstance, node: usize) -> Mat {
    Mat {
        m: instance.node_transforms()[node],
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "kit-shots".to_string()),
    );
    std::fs::create_dir_all(&dir)?;
    let gpu = Gpu::new()?;
    let assets = KitAssets::new()?;
    let pistol = SceneShape::load(&assets, &kit_path("pistol.dts")).ok_or("pistol.dts")?;
    let pistol_pose = pistol.instance().ok_or("no instance")?;
    let mut view = WorldView::new(&gpu);
    view.set_plain_floor(400.0, [150, 152, 156]);
    // The sun behind the camera's usual side, so the front and the right are lit.
    view.set_daylight(&Daylight {
        towards: [0.45, 0.6, 0.66],
        ..Daylight::default()
    });
    for p in PICTURES {
        let shape = SceneShape::load(&assets, &kit_path(p.shape)).ok_or(p.shape)?;
        let s = shape.shape();
        let mut instance = shape.instance().ok_or("no instance")?;
        if let Some((name, pos)) = p.pose {
            let seq = find_sequence(s, name.as_bytes()).ok_or(name)?;
            let t = instance.create_thread(s).ok_or("no thread")?;
            instance.set_sequence(s, t, seq as i32, pos);
        }
        if p.hold {
            // The arm pose of holding, over the rest.
            let seq = find_sequence(s, b"hold").ok_or("hold")?;
            let t = instance.create_thread(s).ok_or("no thread")?;
            instance.set_sequence(s, t, seq as i32, 0.0);
        }
        let detail = shape.detail().ok_or("no detail")?;
        let _ = instance.animate(s, detail, None, &mut |_| true);
        let mut place = Mat::IDENTITY;
        place.set_column3([0.0, 0.0, p.lift]);
        let mut objects = vec![view.shape_object(&assets, &shape, &instance, &place, 1)];
        if p.hold {
            // The pistol's mountPoint on the hand's mount0.
            let mount0 = shape.node("mount0").ok_or("mount0")?;
            let hand = Mat::mul(&place, &node_matrix(&instance, mount0));
            let grip = pistol.node("mountPoint").ok_or("mountPoint")?;
            let held = Mat::mul(&hand, &node_matrix(&pistol_pose, grip).affine_inverse());
            objects.push(view.shape_object(&assets, &pistol, &pistol_pose, &held, 2));
        }
        let b = s.bounds;
        let size = ((b[3] - b[0]).powi(2) + (b[4] - b[1]).powi(2) + (b[5] - b[2]).powi(2)).sqrt();
        for (k, mark) in p.marks.iter().enumerate() {
            let node = shape.node(mark).ok_or(*mark)?;
            let at = place.mul_p(mul_p(&instance.node_transforms()[node], [0.0; 3]));
            let mut o = box_object(10 + k as u32, at, [size * 0.025; 3]);
            for v in o.draws.iter_mut().flat_map(|d| d.vertices.iter_mut()) {
                v.color = MARK_COLOURS[k % MARK_COLOURS.len()];
            }
            objects.push(o);
        }
        view.set_objects(objects);
        // The camera turned `turn` about the shape from its front, far enough to hold it (times `zoom`).
        let centre = p.aim.unwrap_or([
            0.5 * (b[0] + b[3]),
            0.5 * (b[1] + b[4]),
            0.5 * (b[2] + b[5]) + p.lift,
        ]);
        let distance = size.max(0.2) * 1.9 * p.zoom;
        let (s_turn, c_turn) = p.turn.sin_cos();
        let flat = (1.0 - p.rise * p.rise).sqrt();
        let eye_at = [
            centre[0] + distance * s_turn * flat,
            centre[1] + distance * c_turn * flat,
            centre[2] + distance * p.rise,
        ];
        let mut eye = look_at(eye_at, centre);
        eye.fov = Some(FOV);
        view.reset_history();
        let mut frame = None;
        for _ in 0..FRAMES {
            frame = Some(view.render_world(&gpu, SIZE, eye)?);
        }
        let frame = frame.ok_or("no frame")?;
        let width = frame.width;
        let png = torque_example_assets::textures::png(frame.width, frame.height, |x, y| {
            let i = 4 * (y * width + x) as usize;
            [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
        })?;
        let path = dir.join(format!("{}.png", p.name));
        std::fs::write(&path, png)?;
        println!("{}", path.display());
    }
    Ok(())
}
