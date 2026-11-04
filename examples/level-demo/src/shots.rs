//! The example without a window: the menu, then each map played for a moment, every frame drawn offscreen as the window
//! would draw it, and pictures taken: the menu, and for each map an overview from above, a view from behind the figure as it
//! walks in, and the view from its eyes once it stopped.

use std::path::{Path, PathBuf};

use torque::app::shots::{Seat, Shots};
use torque::app::window::offscreen_gpu;
use torque_render_core::Frame;
use torque_render_scene::source::Eye;

use crate::client::{LevelDemo, keys};
use crate::files::GameFiles;

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
    for index in 0..seat.game.maps.len().min(9) {
        let name = map_name(&seat.game.maps[index].path);
        let number = keys::ONE + index as u8;
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
        seat.game.key(keys::W, true);
        seat.run(&gpu, 700)?;
        save(&picture(&mut seat, None)?, &format!("{name}-behind"))?;
        seat.game.key(keys::W, false);
        seat.run(&gpu, 500)?;
        seat.game.set_first_person(true);
        save(&picture(&mut seat, None)?, &format!("{name}-eye"))?;
        seat.game.set_first_person(false);
        seat.game.key(keys::ESCAPE, true);
        seat.game.key(keys::ESCAPE, false);
        seat.run(&gpu, 200)?;
    }
    Ok(shots.paths)
}
