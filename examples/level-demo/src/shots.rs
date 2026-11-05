//! The example without a window: the menu, then each map played for a moment, every frame drawn offscreen as the window
//! would draw it, and pictures taken: the menu, and for each map an overview from above, a view from behind the figure as it
//! walks in, and the view from its eyes once it stopped.

use std::path::{Path, PathBuf};

use torque::app::shots::{Seat, Shots};
use torque::app::window::offscreen_gpu;
use torque::client::input::Key;
use torque_render_core::Frame;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::source::Eye;

use crate::client::LevelDemo;
use crate::files::GameFiles;

/// The keys that pick the first nine maps on the menu.
const DIGITS: [Key; 9] = [
    Key::Digit1,
    Key::Digit2,
    Key::Digit3,
    Key::Digit4,
    Key::Digit5,
    Key::Digit6,
    Key::Digit7,
    Key::Digit8,
    Key::Digit9,
];

/// The longest a map may take to start (ms of the clock).
pub const START_LIMIT_MS: u32 = 20_000;

/// The name a map's pictures start with: its script's name (`missions/plaza.mis` is `plaza`).
pub fn map_name(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map_or_else(|| path.to_string(), |s| s.to_string_lossy().into_owned())
}

/// The pictures at `size`, written into `dir`: `menu.png`, then `<map>-overview.png`, `<map>-behind.png` and
/// `<map>-eye.png` for every map. Returns the paths.
pub fn write_all(files: GameFiles, dir: &Path, size: (u32, u32)) -> Result<Vec<PathBuf>, String> {
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let mut seat = Seat::new(LevelDemo::new(files)?, &gpu, size)?;
    let mut shots = Shots::new(dir)?;
    let mut save = |frame: &Frame, name: &str| shots.save(frame, name);
    // A picture from `eye` (or the game's own eye) after the renderer settled on it.
    let picture = |seat: &mut Seat<LevelDemo>, eye: Option<Eye>| -> Result<Frame, String> {
        seat.eye = eye;
        let frame = seat.picture(&gpu);
        seat.eye = None;
        frame
    };
    seat.run(&gpu, 200)?;
    save(&picture(&mut seat, None)?, "menu")?;
    let count = seat.game.maps.len();
    for (index, &number) in DIGITS.iter().enumerate().take(count) {
        let name = map_name(&seat.game.maps[index].path);
        seat.game.key(number, true);
        seat.game.key(number, false);
        let mut waited = 0;
        while !(seat.game.in_world() && seat.game.figure().is_some()) && waited < START_LIMIT_MS {
            seat.run(&gpu, 100)?;
            waited += 100;
        }
        if !seat.game.in_world() {
            return Err(format!("{name} did not start"));
        }
        // Every cube's ghost (and the map's name gone from the middle of the view), then the overview, then a short walk in.
        seat.run(&gpu, 2800)?;
        let overview = seat.game.overview_eye();
        save(
            &picture(&mut seat, Some(overview))?,
            &format!("{name}-overview"),
        )?;
        // Behind the figure in mid-stride (the clock stands still while the picture settles).
        seat.game.key(Key::W, true);
        seat.run(&gpu, 700)?;
        save(&picture(&mut seat, None)?, &format!("{name}-behind"))?;
        seat.game.key(Key::W, false);
        seat.run(&gpu, 500)?;
        seat.game.set_first_person(true);
        save(&picture(&mut seat, None)?, &format!("{name}-eye"))?;
        seat.game.set_first_person(false);
        seat.game.key(Key::Escape, true);
        seat.game.key(Key::Escape, false);
        seat.run(&gpu, 200)?;
    }
    Ok(shots.paths)
}

/// The office's corner in the map (the generator's numbers run from it).
const OFFICE_CORNER: [f32; 2] = [-18.0, -12.0];

/// The office level's pictures at `size` into `dir`: the outside from above, the figure walked in at the front door (the camera
/// behind it), views from a standing figure's eyes (1.7 units up) of the lobby, the desks, the meeting room and the kitchen, a
/// view out through a window, and the camera behind the figure once it has backed up to the front wall.
pub fn write_office(
    files: GameFiles,
    dir: &Path,
    size: (u32, u32),
) -> Result<Vec<PathBuf>, String> {
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let mut seat = Seat::new(LevelDemo::new(files)?, &gpu, size)?;
    let mut shots = Shots::new(dir)?;
    seat.run(&gpu, 200)?;
    let index = seat
        .game
        .maps
        .iter()
        .position(|m| map_name(&m.path) == "office")
        .ok_or("no office map")?;
    seat.game.start_map(index)?;
    let mut waited = 0;
    while !(seat.game.in_world() && seat.game.figure().is_some()) && waited < START_LIMIT_MS {
        seat.run(&gpu, 100)?;
        waited += 100;
    }
    seat.run(&gpu, 2800)?;
    let at = |x: f32, y: f32, z: f32| [x + OFFICE_CORNER[0], y + OFFICE_CORNER[1], z];
    let eye = |from: [f32; 3], to: [f32; 3]| crate::client::with_fov(look_at(from, to));
    let mut picture =
        |seat: &mut Seat<LevelDemo>, e: Option<Eye>, name: &str| -> Result<(), String> {
            seat.eye = e;
            let frame = seat.picture(&gpu);
            seat.eye = None;
            shots.save(&frame?, name)
        };
    picture(
        &mut seat,
        Some(eye(at(46.0, -16.0, 22.0), at(18.0, 12.0, 0.0))),
        "office-overview",
    )?;
    // In at the front door: the camera behind the figure once it is past the wall.
    seat.game.key(Key::W, true);
    seat.run(&gpu, 1900)?;
    picture(&mut seat, None, "office-walk-in")?;
    seat.run(&gpu, 1600)?;
    seat.game.key(Key::W, false);
    // Aside, then backing up toward the front wall: the camera behind the figure stays inside, in front of the wall.
    seat.game.key(Key::D, true);
    seat.run(&gpu, 1100)?;
    seat.game.key(Key::D, false);
    seat.game.key(Key::S, true);
    seat.run(&gpu, 1500)?;
    seat.game.key(Key::S, false);
    seat.run(&gpu, 300)?;
    picture(&mut seat, None, "office-third-person-wall")?;
    for (name, from, to) in [
        ("office-lobby", at(9.0, 2.0, 1.7), at(2.0, 9.0, 1.0)),
        ("office-desks", at(12.8, 6.4, 1.7), at(30.0, 4.0, 0.8)),
        ("office-meeting", at(15.0, 15.4, 1.7), at(15.5, 22.0, 0.9)),
        ("office-kitchen", at(8.6, 16.2, 1.7), at(2.0, 21.5, 0.8)),
        ("office-window", at(21.0, 1.8, 1.6), at(21.4, -10.0, 1.0)),
    ] {
        picture(&mut seat, Some(eye(from, to)), name)?;
    }
    Ok(shots.paths)
}
