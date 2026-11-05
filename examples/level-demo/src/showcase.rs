//! Pictures of the world alone (no menu, HUD or hints), drawn offscreen at the renderer's highest quality: the valley's
//! views ([`write_valley`]) and the pictures the project's front page shows ([`valley_picture`], [`office_picture`]).

use std::path::{Path, PathBuf};

use torque::app::shots::{Seat, Shots};
use torque_render_core::{Frame, Gpu};
use torque_render_scene::hd::Quality;
use torque_render_scene::hd::kit::look_at;

use crate::client::{LevelDemo, with_fov};
use crate::files::GameFiles;
use crate::shots::{START_LIMIT_MS, map_name};

/// The office's corner in the map (the generator's numbers run from it).
const OFFICE_CORNER: [f32; 2] = [-18.0, -12.0];

/// The valley's views: a name, the eye (x, y and its height over the ground) and what it looks at (x, y, z).
pub const VALLEY_VIEWS: [(&str, [f32; 3], [f32; 3]); 5] = [
    ("valley-shore", [90.0, -95.0, 14.0], [-360.0, 10.0, 40.0]),
    ("valley-high", [340.0, -200.0, 60.0], [-320.0, 0.0, 30.0]),
    ("valley-north", [40.0, 230.0, 20.0], [-330.0, -40.0, 40.0]),
    ("valley-south", [-60.0, -250.0, 15.0], [-330.0, 60.0, 34.0]),
    ("valley-start", [200.0, -34.0, 3.5], [-100.0, -10.0, 34.0]),
];

/// The eye of a view of [`VALLEY_VIEWS`]: its height over the valley's ground made a height.
pub fn valley_eye(at: [f32; 3]) -> [f32; 3] {
    [
        at[0],
        at[1],
        torque_example_assets::valley::height(at[0], at[1]) + at[2],
    ]
}

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

/// The valley's views ([`VALLEY_VIEWS`]) at `size`, written into `dir`. Returns the paths.
pub fn write_valley(
    gpu: &Gpu,
    files: GameFiles,
    dir: &Path,
    size: (u32, u32),
) -> Result<Vec<PathBuf>, String> {
    let mut seat = seat_on(gpu, files, size, "valley")?;
    let mut shots = Shots::new(dir)?;
    for (name, from, to) in VALLEY_VIEWS {
        shots.save(&view(gpu, &mut seat, valley_eye(from), to)?, name)?;
    }
    Ok(shots.paths)
}

/// The front page's valley: the lake at golden hour from its eastern shore, the sun low over the pass.
pub fn valley_picture(gpu: &Gpu, files: GameFiles, size: (u32, u32)) -> Result<Frame, String> {
    let mut seat = seat_on(gpu, files, size, "valley")?;
    let (_, from, to) = VALLEY_VIEWS[0];
    view(gpu, &mut seat, valley_eye(from), to)
}

/// The front page's office: the open-plan desks under their lights, the glass of the meeting room beyond.
pub fn office_picture(gpu: &Gpu, files: GameFiles, size: (u32, u32)) -> Result<Frame, String> {
    let mut seat = seat_on(gpu, files, size, "office")?;
    let at = |x: f32, y: f32, z: f32| [x + OFFICE_CORNER[0], y + OFFICE_CORNER[1], z];
    view(gpu, &mut seat, at(12.8, 6.4, 1.7), at(30.0, 4.0, 0.8))
}
