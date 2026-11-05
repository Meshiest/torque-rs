//! Pictures of the world alone (no menu, HUD or hints), drawn offscreen at the renderer's highest quality: a map's views
//! ([`seat_on`], [`view`]) and the office the project's front page shows ([`office_picture`]).

use torque::app::shots::Seat;
use torque_render_core::{Frame, Gpu};
use torque_render_scene::hd::Quality;
use torque_render_scene::hd::kit::look_at;

use crate::client::{LevelDemo, with_fov};
use crate::files::GameFiles;
use crate::shots::{START_LIMIT_MS, map_name};

/// The office's corner in the map (the generator's numbers run from it).
const OFFICE_CORNER: [f32; 2] = [-18.0, -12.0];

/// The game on `files` drawn at `size` with the highest quality, the map `name` started and its figure there.
pub fn seat_on(
    gpu: &Gpu,
    files: GameFiles,
    size: (u32, u32),
    name: &str,
) -> Result<Seat<LevelDemo>, String> {
    let mut seat = Seat::new(LevelDemo::new(files)?, gpu, size)?;
    seat.view.set_quality(Quality::Ultra);
    seat.run(gpu, 200)?;
    let index = seat
        .game
        .maps
        .iter()
        .position(|m| map_name(&m.path) == name)
        .ok_or_else(|| format!("no {name} map"))?;
    seat.game.start_map(index)?;
    let mut waited = 0;
    while !(seat.game.in_world() && seat.game.figure().is_some()) && waited < START_LIMIT_MS {
        seat.run(gpu, 100)?;
        waited += 100;
    }
    if !seat.game.in_world() {
        return Err(format!("{name} did not start"));
    }
    // Every ghost there and the map's name gone from the middle of the view.
    seat.run(gpu, 3000)?;
    Ok(seat)
}

/// A picture of the world from `from` looking at `to`.
pub fn view(
    gpu: &Gpu,
    seat: &mut Seat<LevelDemo>,
    from: [f32; 3],
    to: [f32; 3],
) -> Result<Frame, String> {
    seat.eye = Some(with_fov(look_at(from, to)));
    let frame = seat.world_picture(gpu);
    seat.eye = None;
    frame
}

/// The front page's office: the open-plan desks under their lights, the glass of the meeting room beyond.
pub fn office_picture(gpu: &Gpu, files: GameFiles, size: (u32, u32)) -> Result<Frame, String> {
    let mut seat = seat_on(gpu, files, size, "office")?;
    let at = |x: f32, y: f32, z: f32| [x + OFFICE_CORNER[0], y + OFFICE_CORNER[1], z];
    view(gpu, &mut seat, at(12.8, 6.4, 1.7), at(30.0, 4.0, 0.8))
}
