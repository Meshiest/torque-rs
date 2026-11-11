//! The catalog's pictures: one icon of each kind of prop the office has, rendered offscreen from the prop's own shape.
//!
//! Each kind's shape (the first item of that kind in `levels/office.bbmodel`, built as the office's props are) is seen by a fixed
//! camera: the same three-quarter direction for every kind, a key light from the front and a little to the left of the picture, and a distance that makes its
//! bounding sphere fit. The renderer has no orthographic projection, so the camera is as far away and its field of view as narrow
//! (`FOV` degrees) as makes perspective vanish. The background is transparent: the prop hangs high above a plain floor (so that its shadow
//! falls out of the picture) that fills the view, each picture is drawn twice over a black and a white floor, and the alpha is what
//! the two differ by. The pictures are `size` pixels square. [`write_icons`] writes
//! `<kind word>.png` for each kind into a folder; the catalog mod's `icons/` folder is where they go.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use torque::app::shots::Seat;
use torque::app::view::{Daylight, SceneShape, WorldView};
use torque::app::window::offscreen_gpu;
use torque::app::{App, Host};
use torque::gui::GuiDraws;
use torque::gui::draw::WorldDraw;
use torque_import::{Options, bbmodel};
use torque_render_core::{Frame, Gpu};
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::math::Mat;
use torque_render_scene::source::Eye;

use crate::client::LevelDemo;
use crate::files::GameFiles;
use crate::props::{KindShape, PropKind, build_kinds, split};

/// The direction the camera looks from, toward the prop's middle (not unit; `x` right, `y` back, `z` up).
const FROM: [f32; 3] = [0.65, 0.8, 0.75];

/// The camera's field of view, degrees: the narrowest the renderer takes, which with the distance it makes is close to orthographic.
const FOV: u8 = 4;

/// How high above the floor the prop hangs (units): its shadow lands well outside the picture.
const LIFT: f32 = 150.0;

/// Toward the key light: in front of the prop and a little to the picture's left, and above, so that the shading falls to the back and
/// the right.
const LIGHT: [f32; 3] = [0.71, 0.33, 0.6];

/// One kind to draw: its shape in the kit and its box.
struct Subject {
    kind: PropKind,
    shape: KindShape,
    half: [f32; 3],
}

/// The first item of each kind in the office's model, built into a scratch copy of the files.
fn subjects(files: &GameFiles) -> Result<Vec<Subject>, String> {
    let mut scratch = GameFiles::bundled().map_err(|e| e.to_string())?;
    let bytes = files
        .file("levels/office.bbmodel")
        .ok_or("the game folder has no levels/office.bbmodel")?;
    let options = Options {
        box_hull: false,
        ..Options::default()
    };
    let model = bbmodel::read(bytes, &options, &mut |_| true).map_err(|e| e.to_string())?;
    let (_, props) = split(model)?;
    let built = build_kinds(&mut scratch, "office", &props)?;
    let mut half: BTreeMap<PropKind, [f32; 3]> = BTreeMap::new();
    for prop in &props {
        half.entry(prop.kind).or_insert(prop.half);
    }
    Ok(built
        .kinds
        .into_iter()
        .filter_map(|shape| {
            let half = half.get(&shape.kind).copied()?;
            Some(Subject {
                kind: shape.kind,
                shape,
                half,
            })
        })
        .collect())
}

/// The daylight of a picture: the sun toward [`LIGHT`], no haze at the prop's distance.
fn sun() -> Daylight {
    Daylight {
        towards: LIGHT,
        fog_start: 1.0e6,
        visible: 2.0e6,
        ..Daylight::default()
    }
}

/// A picture made of two frames of the same prop, one over a black and one over a white floor: its alpha is how little the floor
/// shows through, its color what the prop is without it. The white floor as lit is what the corner of the white frame shows.
fn matte(black: &Frame, white: &Frame) -> Result<Vec<u8>, String> {
    let (width, height) = (black.width, black.height);
    // What the floor alone gives at the corner: the black floor's haze, and how far the white one is from it.
    let haze: [i32; 3] = std::array::from_fn(|c| i32::from(black.rgba[c]));
    let room = (0..3)
        .map(|c| i32::from(white.rgba[c]) - haze[c])
        .sum::<i32>()
        .max(3)
        / 3;
    torque_example_assets::textures::png_rgba(width, height, |x, y| {
        let at = 4 * (y * width + x) as usize;
        let gap = (0..3)
            .map(|c| i32::from(white.rgba[at + c]) - i32::from(black.rgba[at + c]))
            .sum::<i32>()
            / 3;
        // Where the floor shows through entirely the picture is clear; the haze over it is not the prop's.
        let alpha = (255 - gap * 255 / room).clamp(0, 255);
        if alpha < 3 {
            return [0, 0, 0, 0];
        }
        let un = |c: usize| {
            let prop = i32::from(black.rgba[at + c]) - haze[c] * (255 - alpha) / 255;
            (prop * 255 / alpha).clamp(0, 255) as u8
        };
        [un(0), un(1), un(2), alpha as u8]
    })
    .map_err(|e| e.to_string())
}

/// A client drawn with one prop.
struct Studio {
    demo: LevelDemo,
    shape: Option<(SceneShape, torque::shapes::instance::ShapeInstance)>,
    half: [f32; 3],
    /// The picture's side (pixels).
    size: u32,
}

impl App for Studio {
    fn host(&mut self) -> &mut Host {
        &mut self.demo.client.host
    }

    fn host_ref(&self) -> &Host {
        &self.demo.client.host
    }

    fn title(&self) -> String {
        "torque-rs level demo: catalog icons".to_string()
    }

    fn log_name(&self) -> &str {
        "example-level-demo"
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        let mut view = WorldView::new(gpu);
        view.set_daylight(&sun());
        view.set_plain_floor(4000.0, [0, 0, 0]);
        view
    }

    fn advance(&mut self, _ms: u32) {}

    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        let middle = [0.0, 0.0, LIFT + self.half[2]];
        if let Some((shape, instance)) = &self.shape {
            let mut at = Mat::IDENTITY;
            at.set_column3(middle);
            let object = view.shape_object(&self.demo.files.kit, shape, instance, &at, 1);
            view.set_objects(vec![object]);
        }
        let radius = self.half.iter().map(|h| h * h).sum::<f32>().sqrt();
        let length = FROM.iter().map(|c| c * c).sum::<f32>().sqrt();
        // Far enough that the sphere fits the narrow field with a margin.
        let distance = radius / (f32::from(FOV) / 2.0).to_radians().tan() * 1.15;
        let from = std::array::from_fn(|k| middle[k] + FROM[k] / length * distance);
        // The world fills the picture: a draw list with the one rectangle.
        let size = self.size as i32;
        let draws = GuiDraws {
            size: (self.size, self.size),
            worlds: vec![WorldDraw {
                at_batch: 0,
                rect: [0, 0, size, size],
                control: [0, 0, size, size],
            }],
            ..GuiDraws::default()
        };
        let mut eye = look_at(from, middle);
        eye.fov = Some(FOV);
        (draws, eye)
    }
}

/// The icons of the office's kinds, `size` pixels square, written into `dir` as `<kind word>.png`. Returns the paths.
pub fn write_icons(dir: &Path, size: u32) -> Result<Vec<PathBuf>, String> {
    let files = GameFiles::bundled().map_err(|e| e.to_string())?;
    let subjects = subjects(&files)?;
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let demo = LevelDemo::new(files)?;
    let mut seat = Seat::new(
        Studio {
            demo,
            shape: None,
            half: [0.5; 3],
            size,
        },
        &gpu,
        (size, size),
    )?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    for subject in subjects {
        let shape = SceneShape::load(&seat.game.demo.files.kit, &subject.shape.shape)
            .ok_or_else(|| format!("{}: the shape does not load", subject.shape.shape))?;
        let instance = shape.instance().ok_or("the shape has no instance")?;
        seat.game.shape = Some((shape, instance));
        seat.game.half = subject.half;
        seat.view.set_plain_floor(4000.0, [0, 0, 0]);
        let black = seat.picture(&gpu)?;
        seat.view.set_plain_floor(4000.0, [255, 255, 255]);
        let white = seat.picture(&gpu)?;
        let path = dir.join(format!("{}.png", subject.kind.word()));
        std::fs::write(&path, matte(&black, &white)?).map_err(|e| e.to_string())?;
        paths.push(path);
    }
    Ok(paths)
}
