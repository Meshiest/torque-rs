//! The example without a window: a scripted run of keys and pauses, every frame drawn offscreen as the window would draw it,
//! and pictures taken where the script says.

use std::path::Path;

use torque::app::shots::Shots;
use torque::app::view::WorldView;
use torque::app::window::offscreen_gpu;
use torque_render_core::{Frame, Gpu};

use crate::game::{CharacterGame, keys};

pub use torque::app::shots::FRAME_MS;

/// One step of a scripted run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// A key (a virtual key code) going down (true) or up.
    Key(u8, bool),
    /// Milliseconds of frames.
    Wait(u32),
    /// A picture of the last frame, named.
    Shot(&'static str),
}

/// The run that shows each state: standing, walking sideways past the camera, then a jump while walking.
pub const STATES: &[Step] = &[
    Step::Wait(1000),
    Step::Shot("idle"),
    Step::Key(keys::D, true),
    Step::Wait(800),
    Step::Shot("walk"),
    Step::Key(keys::SPACE, true),
    Step::Wait(300),
    Step::Shot("jump"),
    Step::Key(keys::SPACE, false),
    Step::Key(keys::D, false),
];

/// The camera the pictures are taken from: in front of the figure and to its side, a little above.
pub fn picture_camera(game: &mut CharacterGame) {
    game.orbit.yaw = std::f32::consts::PI + 0.6;
    game.orbit.pitch = 0.08;
    game.orbit.distance = 6.5;
}

/// Plays `script` on `game`, drawing every frame into a target of `size` through `view`; returns the pictures taken.
pub fn play(
    game: &mut CharacterGame,
    gpu: &Gpu,
    view: &mut WorldView,
    size: (u32, u32),
    script: &[Step],
) -> Result<Vec<(&'static str, Frame)>, String> {
    game.host.surface_size(size);
    let target = gpu.offscreen(size.0, size.1).map_err(|e| e.to_string())?;
    let mut shots = Vec::new();
    for step in script {
        match *step {
            Step::Key(vk, down) => game.key(vk, down),
            Step::Wait(ms) => {
                for _ in 0..ms.div_ceil(FRAME_MS) {
                    game.step(FRAME_MS);
                    let (gui, eye) = game.frame(view);
                    view.draw(gpu, &target.color, &gui, eye)?;
                }
            }
            Step::Shot(name) => {
                shots.push((name, gpu.read_back(&target).map_err(|e| e.to_string())?));
            }
        }
    }
    Ok(shots)
}

/// The pictures of [`STATES`] at `size`, written into `dir` as `<state>.png`; returns the paths.
pub fn write_states(dir: &Path, size: (u32, u32)) -> Result<Vec<std::path::PathBuf>, String> {
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let mut game = CharacterGame::new()?;
    picture_camera(&mut game);
    let mut view = game.view(&gpu);
    let mut shots = Shots::new(dir)?;
    for (name, frame) in play(&mut game, &gpu, &mut view, size, STATES)? {
        shots.save(&frame, name)?;
    }
    Ok(shots.paths)
}
