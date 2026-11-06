//! The example without a window: a scripted run of keys and pauses, every frame drawn offscreen as the window would draw it,
//! and pictures taken where the script says.

use std::path::Path;

use torque::app::shots::Shots;
use torque::app::view::WorldView;
use torque::app::window::offscreen_gpu;
use torque::client::input::Key;
use torque_render_core::{Frame, Gpu};

use crate::game::CharacterGame;

pub use torque::app::shots::FRAME_MS;

/// One step of a scripted run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// A key going down (true) or up.
    Key(Key, bool),
    /// Milliseconds of frames.
    Wait(u32),
    /// A picture of the last frame, named.
    Shot(&'static str),
    /// A picture of the world alone as the last frame saw it (no hints over it, over the whole picture), named.
    WorldShot(&'static str),
}

/// The run that shows each state: standing, walking sideways past the camera, then a jump while walking.
pub const STATES: &[Step] = &[
    Step::Wait(1000),
    Step::Shot("idle"),
    Step::Key(Key::D, true),
    Step::Wait(800),
    Step::Shot("walk"),
    Step::Key(Key::Space, true),
    Step::Wait(300),
    Step::Shot("jump"),
    Step::Key(Key::Space, false),
    Step::Key(Key::D, false),
];

/// [`STATES`]' run with pictures of the world alone (the front page's).
pub const WORLD_STATES: &[Step] = &[
    Step::Wait(1000),
    Step::WorldShot("idle"),
    Step::Key(Key::D, true),
    Step::Wait(800),
    Step::WorldShot("walk"),
    Step::Key(Key::Space, true),
    Step::Wait(300),
    Step::WorldShot("jump"),
    Step::Key(Key::Space, false),
    Step::Key(Key::D, false),
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
    let mut last_eye = None;
    // The real clock the frames are counted by (the corner readout's frame rate), as a window counts its own.
    let clock = std::time::Instant::now();
    for step in script {
        match *step {
            Step::Key(key, down) => game.key(key, down),
            Step::Wait(ms) => {
                for _ in 0..ms.div_ceil(FRAME_MS) {
                    game.step(FRAME_MS);
                    let (gui, eye) = game.frame(view);
                    view.draw(gpu, &target.color, &gui, eye)?;
                    game.host
                        .frame_drawn(clock.elapsed().as_secs_f64() * 1000.0);
                    last_eye = Some(eye);
                }
            }
            Step::Shot(name) => {
                shots.push((name, gpu.read_back(&target).map_err(|e| e.to_string())?));
            }
            Step::WorldShot(name) => {
                let eye = last_eye.ok_or("no frame drawn before the picture")?;
                view.draw_world(&target.color, [0, 0, size.0, size.1], eye)?;
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
