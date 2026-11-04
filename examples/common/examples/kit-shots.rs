//! Pictures of every shape of the asset kit for review, drawn offscreen on a plain grey floor: the cube, the figure from the
//! front, side and back and in its idle, walk, run and jump poses, the pistol with its muzzle and eject points marked, and the
//! projectile.
//!
//! `cargo run -p torque-example-common --example kit-shots -- <dir>` (default `kit-shots`)

use std::f32::consts::{FRAC_PI_2, PI};
use std::path::PathBuf;

use torque::shapes::instance::find_sequence;
use torque::shapes::math::mul_p;
use torque_example_common::view::{SceneShape, WorldView};
use torque_example_common::{KitAssets, kit_path};
use torque_render_core::Gpu;
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::math::Mat;

const SIZE: (u32, u32) = (800, 600);
/// Frames drawn for each picture (the anti-aliasing's history settles over them).
const FRAMES: usize = 12;
const FOV: u8 = 50;

/// One picture: the shape, a sequence and its position (None: the rest pose), how high its origin stands, the camera's turn
/// about the shape (0: in front of it, looking along -y at its face) and the nodes to mark.
struct Picture {
    name: &'static str,
    shape: &'static str,
    pose: Option<(&'static str, f32)>,
    lift: f32,
    turn: f32,
    marks: &'static [&'static str],
}

const PICTURES: &[Picture] = &[
    Picture {
        name: "cube",
        shape: "cube.dts",
        pose: None,
        lift: 0.5,
        turn: 0.6,
        marks: &[],
    },
    Picture {
        name: "figure-front",
        shape: "figure.dts",
        pose: None,
        lift: 0.0,
        turn: 0.0,
        marks: &[],
    },
    Picture {
        name: "figure-side",
        shape: "figure.dts",
        pose: None,
        lift: 0.0,
        turn: FRAC_PI_2,
        marks: &[],
    },
    Picture {
        name: "figure-back",
        shape: "figure.dts",
        pose: None,
        lift: 0.0,
        turn: PI,
        marks: &[],
    },
    Picture {
        name: "figure-idle",
        shape: "figure.dts",
        pose: Some(("idle", 0.25)),
        lift: 0.0,
        turn: 0.7,
        marks: &[],
    },
    Picture {
        name: "figure-walk",
        shape: "figure.dts",
        pose: Some(("walk", 0.25)),
        lift: 0.0,
        turn: FRAC_PI_2,
        marks: &[],
    },
    Picture {
        name: "figure-run",
        shape: "figure.dts",
        pose: Some(("run", 0.25)),
        lift: 0.0,
        turn: FRAC_PI_2,
        marks: &[],
    },
    Picture {
        name: "figure-jump",
        shape: "figure.dts",
        pose: Some(("jump", 1.0)),
        lift: 0.3,
        turn: 0.9,
        marks: &[],
    },
    Picture {
        name: "pistol",
        shape: "pistol.dts",
        pose: None,
        lift: 0.25,
        turn: 1.9,
        marks: &["muzzlePoint", "ejectPoint", "mountPoint"],
    },
    Picture {
        name: "projectile",
        shape: "projectile.dts",
        pose: None,
        lift: 0.15,
        turn: 1.9,
        marks: &[],
    },
];

/// The marks' colours, in the order of a picture's marks.
const MARK_COLOURS: [[u8; 4]; 3] = [[230, 50, 40, 255], [40, 120, 230, 255], [60, 190, 70, 255]];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "kit-shots".to_string()),
    );
    std::fs::create_dir_all(&dir)?;
    let gpu = Gpu::new()?;
    let assets = KitAssets::new()?;
    let mut view = WorldView::new(&gpu);
    view.set_plain_floor(400.0, [150, 152, 156]);
    for p in PICTURES {
        let shape = SceneShape::load(&assets, &kit_path(p.shape)).ok_or(p.shape)?;
        let s = shape.shape();
        let mut instance = shape.instance().ok_or("no instance")?;
        if let Some((name, pos)) = p.pose {
            let seq = find_sequence(s, name.as_bytes()).ok_or(name)?;
            let t = instance.create_thread(s).ok_or("no thread")?;
            instance.set_sequence(s, t, seq as i32, pos);
        }
        let detail = shape.detail().ok_or("no detail")?;
        let _ = instance.animate(s, detail, None, &mut |_| true);
        let mut place = Mat::IDENTITY;
        place.set_column3([0.0, 0.0, p.lift]);
        let mut objects = vec![view.shape_object(&assets, &shape, &instance, &place, 1)];
        let b = s.bounds;
        let size = ((b[3] - b[0]).powi(2) + (b[4] - b[1]).powi(2) + (b[5] - b[2]).powi(2)).sqrt();
        for (k, mark) in p.marks.iter().enumerate() {
            let node = shape.node(mark).ok_or(*mark)?;
            let at = place.mul_p(mul_p(&instance.node_transforms()[node], [0.0; 3]));
            let mut o = box_object(10 + k as u32, at, [size * 0.03; 3]);
            for v in o.draws.iter_mut().flat_map(|d| d.vertices.iter_mut()) {
                v.color = MARK_COLOURS[k % MARK_COLOURS.len()];
            }
            objects.push(o);
        }
        view.set_objects(objects);
        // The camera in front of the shape (+y, its face) turned `turn` about it, a little above, far enough to hold it.
        let centre = [
            0.5 * (b[0] + b[3]),
            0.5 * (b[1] + b[4]),
            0.5 * (b[2] + b[5]) + p.lift,
        ];
        let distance = size.max(0.2) * 1.9;
        let (s_turn, c_turn) = p.turn.sin_cos();
        let eye_at = [
            centre[0] + distance * s_turn * 0.94,
            centre[1] + distance * c_turn * 0.94,
            centre[2] + distance * 0.34,
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
