//! The example without a window: the menu, then each map played for a moment, every frame drawn offscreen as the window
//! would draw it, and pictures taken: the menu, and for each map an overview from above, a view from behind the figure as it
//! walks in, and the view from its eyes once it stopped.

use std::io;
use std::path::{Path, PathBuf};

use torque_example_common::view::WorldView;
use torque_render_core::{Frame, Gpu};
use torque_render_scene::source::Eye;

use crate::client::{LevelDemo, keys};
use crate::files::GameFiles;

/// The clock's step between frames (ms).
pub const FRAME_MS: u32 = 16;
/// The longest a map may take to start (ms of the clock).
pub const START_LIMIT_MS: u32 = 20_000;
/// The frames drawn from a still camera before its picture (the renderer's history settles).
pub const SETTLE_FRAMES: u32 = 24;

/// What the pictures draw into.
struct Shooter<'a> {
    gpu: &'a Gpu,
    view: WorldView,
    target: torque_render_core::Offscreen,
}

impl Shooter<'_> {
    /// Draws one frame of `game`; with `eye` the frame is seen from there instead of the game's own eye.
    fn frame(&mut self, game: &mut LevelDemo, eye: Option<Eye>) -> Result<(), String> {
        let (gui, own) = game.frame(&mut self.view);
        self.view
            .draw(self.gpu, &self.target.color, &gui, eye.unwrap_or(own))?;
        Ok(())
    }

    /// Runs `ms` of the clock, a frame every [`FRAME_MS`].
    fn run(&mut self, game: &mut LevelDemo, ms: u32) -> Result<(), String> {
        for _ in 0..ms.div_ceil(FRAME_MS) {
            game.step(FRAME_MS);
            game.add_frame_time(FRAME_MS as f32 / 1000.0);
            self.frame(game, None)?;
        }
        Ok(())
    }

    /// A picture from `eye` (or the game's own eye) after the renderer settled on it.
    fn picture(&mut self, game: &mut LevelDemo, eye: Option<Eye>) -> Result<Frame, String> {
        self.view.reset_history();
        for _ in 0..SETTLE_FRAMES {
            self.frame(game, eye)?;
        }
        self.gpu.read_back(&self.target).map_err(|e| e.to_string())
    }
}

/// Writes `frame` as a PNG.
pub fn write_png(frame: &Frame, path: &Path) -> io::Result<()> {
    let width = frame.width;
    let bytes = torque_example_assets::textures::png(frame.width, frame.height, |x, y| {
        let i = 4 * (y * width + x) as usize;
        [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
    })?;
    std::fs::write(path, bytes)
}

/// The name a map's pictures start with: its script's name (`missions/plaza.mis` is `plaza`).
pub fn map_name(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map_or_else(|| path.to_string(), |s| s.to_string_lossy().into_owned())
}

/// The pictures at `size`, written into `dir`: `menu.png`, then `<map>-overview.png`, `<map>-behind.png` and
/// `<map>-eye.png` for every map. Returns the paths.
pub fn write_all(files: GameFiles, dir: &Path, size: (u32, u32)) -> Result<Vec<PathBuf>, String> {
    let gpu = torque_example_common::window::offscreen_gpu().map_err(|e| e.to_string())?;
    let mut game = LevelDemo::new(files)?;
    game.host.surface_size(size);
    let mut shooter = Shooter {
        gpu: &gpu,
        view: game.view(&gpu),
        target: gpu.offscreen(size.0, size.1).map_err(|e| e.to_string())?,
    };
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    let mut save = |frame: &Frame, name: &str| -> Result<(), String> {
        let path = dir.join(format!("{name}.png"));
        write_png(frame, &path).map_err(|e| e.to_string())?;
        paths.push(path);
        Ok(())
    };
    shooter.run(&mut game, 200)?;
    save(&shooter.picture(&mut game, None)?, "menu")?;
    for index in 0..game.maps.len().min(9) {
        let name = map_name(&game.maps[index].path);
        let number = keys::ONE + index as u8;
        game.key(number, true);
        game.key(number, false);
        let mut waited = 0;
        while !(game.in_world() && game.figure().is_some()) && waited < START_LIMIT_MS {
            shooter.run(&mut game, 100)?;
            waited += 100;
        }
        if !game.in_world() {
            return Err(format!("{name} did not start"));
        }
        // Every cube's ghost (and the map's name gone from the middle of the view), then the overview, then a short walk in.
        shooter.run(&mut game, 2800)?;
        let overview = game.overview_eye();
        save(
            &shooter.picture(&mut game, Some(overview))?,
            &format!("{name}-overview"),
        )?;
        // Behind the figure in mid-stride (the clock stands still while the picture settles).
        game.key(keys::W, true);
        shooter.run(&mut game, 700)?;
        save(
            &shooter.picture(&mut game, None)?,
            &format!("{name}-behind"),
        )?;
        game.key(keys::W, false);
        shooter.run(&mut game, 500)?;
        game.set_first_person(true);
        save(&shooter.picture(&mut game, None)?, &format!("{name}-eye"))?;
        game.set_first_person(false);
        game.key(keys::ESCAPE, true);
        game.key(keys::ESCAPE, false);
        shooter.run(&mut game, 200)?;
    }
    Ok(paths)
}
