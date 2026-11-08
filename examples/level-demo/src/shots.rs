//! The example without a window: the menu, then each map played for a moment, every frame drawn offscreen as the window
//! would draw it, and pictures taken: the menu, and for each map an overview from above, a view from behind the figure as it
//! walks in, and the view from its eyes once it stopped.

use std::path::{Path, PathBuf};

#[cfg(feature = "rope")]
use torque::app::App;
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
        // Esc, then the pause menu's Leave game: back to the menu of maps.
        seat.game.key(Key::Escape, true);
        seat.game.key(Key::Escape, false);
        seat.run(&gpu, 64)?;
        seat.game.call("leaveGame", &[])?;
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

/// The plaza's rope (the `rope` feature) at `size` into `dir`: the camera behind the figure as it walks up to the stage (`rope-behind`),
/// the line from the front, close (`rope-front`), and the line thrown up and falling back, six pictures a tenth of a second apart
/// (`rope-bounce-0` to `rope-bounce-5`, the clock standing still while each settles) and a push from the side (`rope-swing-0` to
/// `rope-swing-5`).
#[cfg(feature = "rope")]
pub fn write_rope(files: GameFiles, dir: &Path, size: (u32, u32)) -> Result<Vec<PathBuf>, String> {
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let mut seat = start_plaza(files, &gpu, size)?;
    let mut shots = Shots::new(dir)?;
    let line = crate::ropes::line(&seat.game);
    if line == 0 {
        return Err("the plaza hung no rope".into());
    }
    let mut picture =
        |seat: &mut Seat<LevelDemo>, e: Option<Eye>, name: &str| -> Result<(), String> {
            seat.eye = e;
            let frame = seat.picture(&gpu);
            seat.eye = None;
            shots.save(&frame?, name)
        };
    // Up to the stage: the camera behind the figure, the line over it.
    seat.game.key(Key::W, true);
    seat.run(&gpu, 2600)?;
    seat.game.key(Key::W, false);
    seat.run(&gpu, 600)?;
    picture(&mut seat, None, "rope-behind")?;
    let front = Some(crate::client::with_fov(look_at(
        [-6.5, -9.0, 2.2],
        [0.0, -2.0, 3.0],
    )));
    picture(&mut seat, front, "rope-front")?;
    let call = |seat: &mut Seat<LevelDemo>, script: String| -> Result<(), String> {
        seat.game
            .client
            .vm_mut()
            .eval(script.as_bytes())
            .map(drop)
            .map_err(|e| format!("{e:?}"))
    };
    // Thrown up, then falling back.
    call(&mut seat, format!("ropeImpulse({line}, \"0 0 12\");"))?;
    for n in 0..6 {
        seat.run(&gpu, 100)?;
        picture(&mut seat, front, &format!("rope-bounce-{n}"))?;
    }
    seat.run(&gpu, 6000)?;
    // Pushed from the side, and let swing.
    call(&mut seat, format!("ropeImpulse({line}, \"0 14 0\");"))?;
    for n in 0..6 {
        seat.run(&gpu, 200)?;
        picture(&mut seat, front, &format!("rope-swing-{n}"))?;
    }
    Ok(shots.paths)
}

/// The game on the plaza (the `rope` feature), its clothesline hung, the figure standing at the spawn.
#[cfg(feature = "rope")]
fn start_plaza(
    files: GameFiles,
    gpu: &torque_render_core::Gpu,
    size: (u32, u32),
) -> Result<Seat<LevelDemo>, String> {
    let mut seat = Seat::new(LevelDemo::new(files)?, gpu, size)?;
    seat.run(gpu, 200)?;
    let index = seat
        .game
        .maps
        .iter()
        .position(|m| map_name(&m.path) == crate::ropes::MAP)
        .ok_or("no plaza map")?;
    seat.game.start_map(index)?;
    let mut waited = 0;
    while !(seat.game.in_world() && seat.game.figure().is_some()) && waited < START_LIMIT_MS {
        seat.run(gpu, 100)?;
        waited += 100;
    }
    seat.run(gpu, 2800)?;
    Ok(seat)
}

/// The rope styles at `size` into `dir` (the `rope` feature): the plaza's red tube clothesline beside a yellow ribbon cable and
/// glowing red laser lines (`styles-plaza`), the same from above (`styles-above`), the figure with a laser through it and behind
/// it and a ribbon across in front (`styles-crossing`), the camera low at the end of a long ribbon, looking almost along it
/// (`styles-along`), and a thin laser seen from far away (`styles-far`).
#[cfg(feature = "rope")]
pub fn write_rope_styles(
    files: GameFiles,
    dir: &Path,
    size: (u32, u32),
) -> Result<Vec<PathBuf>, String> {
    use crate::ropes::{declare_styles, make};
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let mut seat = start_plaza(files, &gpu, size)?;
    let mut shots = Shots::new(dir)?;
    declare_styles(&mut seat.game)?;
    let mut picture = |seat: &mut Seat<LevelDemo>, eye: Eye, name: &str| -> Result<(), String> {
        seat.eye = Some(crate::client::with_fov(eye));
        let frame = seat.picture(&gpu);
        seat.eye = None;
        shots.save(&frame?, name)
    };
    let clear = |seat: &mut Seat<LevelDemo>| -> Result<(), String> {
        seat.game
            .client
            .vm_mut()
            .eval(b"ropeClear();")
            .map(drop)
            .map_err(|e| format!("{e:?}"))
    };

    // The clothesline (a tube), a ribbon cable in front of the pillars and three lasers across the way between them.
    let [x0, y, z] = crate::ropes::START;
    let x1 = crate::ropes::END[0];
    make(
        &mut seat.game,
        "PlazaRibbon",
        [x0, y - 0.7, z - 0.4],
        [x1, y - 0.7, z - 0.4],
    )?;
    for height in [0.45, 0.9, 1.35] {
        make(
            &mut seat.game,
            "PlazaLaser",
            [x0, y - 0.7, height],
            [x1, y - 0.7, height],
        )?;
    }
    make(
        &mut seat.game,
        "PlazaLaser",
        [x0, y - 0.7, 0.2],
        [x1, y - 3.5, 1.6],
    )?;
    seat.run(&gpu, 6000)?;
    picture(
        &mut seat,
        look_at([-6.5, -10.5, 2.0], [0.0, -2.0, 2.0]),
        "styles-plaza",
    )?;
    picture(
        &mut seat,
        look_at([-4.0, -8.0, 9.0], [0.0, -2.5, 1.5]),
        "styles-above",
    )?;

    // The figure with a laser through its middle, another behind it (as the camera sees it), and a ribbon in front of it.
    clear(&mut seat)?;
    let at = seat.game.figure().ok_or("the figure is gone")?.position;
    let [fx, fy, fz] = at;
    make(
        &mut seat.game,
        "PlazaLaser",
        [fx - 3.5, fy, fz + 0.9],
        [fx + 3.5, fy, fz + 0.9],
    )?;
    make(
        &mut seat.game,
        "PlazaLaser",
        [fx - 3.5, fy - 0.8, fz + 1.4],
        [fx + 3.5, fy - 0.8, fz + 1.4],
    )?;
    make(
        &mut seat.game,
        "PlazaRibbon",
        [fx - 3.5, fy + 0.8, fz + 1.6],
        [fx + 3.5, fy + 0.8, fz + 1.6],
    )?;
    seat.run(&gpu, 6000)?;
    picture(
        &mut seat,
        look_at([fx + 2.2, fy + 4.2, fz + 1.5], [fx, fy, fz + 1.0]),
        "styles-crossing",
    )?;
    picture(
        &mut seat,
        look_at([fx - 3.0, fy + 2.5, fz + 0.5], [fx, fy, fz + 1.0]),
        "styles-crossing-low",
    )?;

    // A long ribbon, the camera low at its end looking almost along it.
    clear(&mut seat)?;
    make(
        &mut seat.game,
        "PlazaRibbon",
        [-3.0, -12.0, 3.4],
        [-3.0, 3.0, 3.4],
    )?;
    seat.run(&gpu, 6000)?;
    picture(
        &mut seat,
        look_at([-2.7, -14.5, 3.1], [-3.0, 3.0, 2.4]),
        "styles-along",
    )?;
    picture(
        &mut seat,
        look_at([-3.0, -14.5, 3.4], [-3.0, 3.0, 3.0]),
        "styles-along-exact",
    )?;
    picture(
        &mut seat,
        look_at([-2.8, -14.5, 4.8], [-3.0, 3.0, 1.8]),
        "styles-along-high",
    )?;

    // A thin laser seen from far away keeps its width on the screen.
    clear(&mut seat)?;
    make(&mut seat.game, "PlazaLaser", [x0, y, 2.0], [x1, y, 2.0])?;
    seat.run(&gpu, 1000)?;
    picture(
        &mut seat,
        look_at([0.0, -45.0, 3.0], [0.0, -2.0, 2.0]),
        "styles-far",
    )?;

    // Ropes that cast: the shadow bands on the ground, from two places.
    clear(&mut seat)?;
    make(
        &mut seat.game,
        "ShadowRibbon",
        [-5.0, -3.0, 3.6],
        [5.0, -3.0, 3.6],
    )?;
    make(
        &mut seat.game,
        "ShadowLine",
        [-5.0, -6.0, 2.8],
        [5.0, -6.0, 2.8],
    )?;
    seat.run(&gpu, 6000)?;
    picture(
        &mut seat,
        look_at([-7.0, -13.0, 6.0], [0.0, -4.5, 0.5]),
        "shadows-cast",
    )?;
    picture(
        &mut seat,
        look_at([9.0, -11.0, 5.0], [0.0, -4.5, 0.5]),
        "shadows-cast-moved",
    )?;

    // Ropes that take a shadow: four lie across the figure's shadow, a tube and a ribbon that take it, each followed by one that
    // does not.
    clear(&mut seat)?;
    let [fx, fy, fz] = at;
    for (name, dy) in [
        ("PlazaTube", 0.3),
        ("PlazaTubeLit", 1.0),
        ("PlazaRibbon", 1.7),
        ("PlazaRibbonLit", 2.4),
    ] {
        make(
            &mut seat.game,
            name,
            [fx - 1.5, fy + dy, fz + 0.3],
            [fx + 6.0, fy + dy, fz + 0.3],
        )?;
    }
    seat.run(&gpu, 6000)?;
    picture(
        &mut seat,
        look_at([fx + 1.5, fy - 3.0, fz + 5.5], [fx + 1.5, fy + 1.4, fz]),
        "shadows-receive",
    )?;

    // Textures: a braided tube and a chain ribbon with holes between its links, from the front of the plaza.
    clear(&mut seat)?;
    let [x0, y, z] = crate::ropes::START;
    let x1 = crate::ropes::END[0];
    make(
        &mut seat.game,
        "PlazaBraid",
        [x0, y - 0.7, z - 0.2],
        [x1, y - 0.7, z - 0.2],
    )?;
    make(
        &mut seat.game,
        "PlazaChain",
        [x0, y - 0.7, z + 0.7],
        [x1, y - 0.7, z + 0.7],
    )?;
    seat.run(&gpu, 6000)?;
    picture(
        &mut seat,
        look_at([-6.5, -10.5, 2.4], [0.0, -2.0, 2.6]),
        "textures-braid-chain",
    )?;
    picture(
        &mut seat,
        look_at([-5.0, -5.0, 2.9], [-7.4, -2.8, 3.1]),
        "textures-braid-chain-close",
    )?;

    // A scrolling laser line, at two moments.
    clear(&mut seat)?;
    make(
        &mut seat.game,
        "PlazaBeam",
        [x0, y - 0.7, 1.4],
        [x1, y - 0.7, 2.6],
    )?;
    seat.run(&gpu, 1000)?;
    let beam_eye = look_at([-6.0, -9.0, 2.0], [0.0, -2.0, 2.0]);
    picture(&mut seat, beam_eye, "textures-beam-scroll-a")?;
    seat.run(&gpu, 200)?;
    picture(&mut seat, beam_eye, "textures-beam-scroll-b")?;

    // Two ropes of one datablock, with a runtime color and texture of their own each.
    clear(&mut seat)?;
    seat.game
        .client
        .vm_mut()
        .eval(
            br#"$left = ropeCreate("-8 -2.7 3.2", "-1 -2.7 2.4", PlazaTwin);
            ropeSetColor($left, "255 90 60 255");
            $right = ropeCreate("1 -2.7 2.4", "8 -2.7 3.2", PlazaTwin);
            ropeSetColor($right, "90 170 255 255");
            ropeSetTexture($right, "rope/chain.png");"#,
        )
        .map_err(|e| format!("{e:?}"))?;
    seat.run(&gpu, 6000)?;
    picture(
        &mut seat,
        look_at([0.0, -10.0, 2.4], [0.0, -2.0, 2.6]),
        "setters-two-ropes",
    )?;

    // Frames at about 120 a second: a rope from the figure's hand to a point that moves, the figure walking. The ends follow what
    // is drawn in each frame, not the game's tick.
    clear(&mut seat)?;
    {
        use torque_plugin_rope::{RopeEnd, RopeParams, RopeShared};
        let ropes = seat
            .game
            .engine()
            .resource::<RopeShared>()
            .ok_or("no rope plugin")?
            .clone();
        let ghost = seat.game.figure().ok_or("the figure is gone")?.ghost;
        let [fx, fy, fz] = at;
        let moving = |t: f32| {
            [
                fx + 2.6 + 0.9 * (t * 5.0).sin(),
                fy + 2.2,
                fz + 1.3 + 0.5 * (t * 5.0).cos(),
            ]
        };
        let id = {
            let mut world = ropes.lock();
            let id = world
                .create(
                    RopeParams {
                        segments: 14,
                        stiffness: 0.3,
                        thickness: 0.07,
                        color: [230, 190, 60, 255],
                        ..RopeParams::default()
                    },
                    [fx, fy, fz + 1.0],
                    moving(0.0),
                )
                .map_err(|e| e.to_string())?;
            world
                .pin_ghost(id, RopeEnd::Start, ghost, "hand")
                .map_err(|e| e.to_string())?;
            id
        };
        seat.view.set_effect_frame_seconds(Some(0.008));
        seat.eye = Some(crate::client::with_fov(look_at(
            [fx + 3.5, fy - 3.5, fz + 2.4],
            [fx + 1.3, fy + 1.0, fz + 1.0],
        )));
        seat.game.key(Key::D, true);
        let mut clock = 0.0f32;
        for frame in 0..(60 + 8) {
            App::advance(&mut seat.game, 8);
            clock += 0.008;
            ropes
                .lock()
                .pin_point(id, RopeEnd::End, moving(clock))
                .map_err(|e| e.to_string())?;
            seat.frame(&gpu)?;
            if frame >= 60 {
                let image = gpu.read_back(&seat.target).map_err(|e| e.to_string())?;
                shots.save(&image, &format!("frames-hand-{}", frame - 60))?;
            }
        }
        seat.game.key(Key::D, false);
        seat.eye = None;
        seat.view.set_effect_frame_seconds(Some(0.016));
    }
    Ok(shots.paths)
}

/// The valley's swimming pictures at `size` into `dir`: the figure walked from the spawn into the lake until it swims, then
/// seen from beside and ahead of it mid-stroke (`swim-stroke`) and from the camera behind it (`swim-behind`); stopped and
/// treading water, from ahead (`swim-tread`).
pub fn write_swim(files: GameFiles, dir: &Path, size: (u32, u32)) -> Result<Vec<PathBuf>, String> {
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let mut seat = Seat::new(LevelDemo::new(files)?, &gpu, size)?;
    let mut shots = Shots::new(dir)?;
    seat.run(&gpu, 200)?;
    let index = seat
        .game
        .maps
        .iter()
        .position(|m| map_name(&m.path) == "valley")
        .ok_or("no valley map")?;
    seat.game.start_map(index)?;
    let mut waited = 0;
    while !(seat.game.in_world() && seat.game.figure().is_some()) && waited < START_LIMIT_MS {
        seat.run(&gpu, 100)?;
        waited += 100;
    }
    seat.run(&gpu, 2000)?;
    // West from the spawn, sprinting, into the lake until the figure swims, then on into deeper water.
    seat.game.key(Key::W, true);
    seat.game.key(Key::ShiftLeft, true);
    let mut waited = 0;
    while !seat.game.swimming() && waited < 40_000 {
        seat.run(&gpu, 100)?;
        waited += 100;
    }
    if !seat.game.swimming() {
        return Err("the figure never swam".into());
    }
    seat.run(&gpu, 5000)?;
    let place = |seat: &Seat<LevelDemo>| seat.game.figure().map(|f| f.position);
    let from = place(&seat).ok_or("no figure")?;
    seat.run(&gpu, 600)?;
    let to = place(&seat).ok_or("no figure")?;
    // The way it swims (flat), and its right.
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let len = dx.hypot(dy).max(1e-3);
    let ahead = [dx / len, dy / len];
    let right = [ahead[1], -ahead[0]];
    let eye_beside = |p: [f32; 3], side: f32, front: f32, up: f32, aim: f32| {
        let from = [
            p[0] + right[0] * side + ahead[0] * front,
            p[1] + right[1] * side + ahead[1] * front,
            p[2] + up,
        ];
        crate::client::with_fov(look_at(from, [p[0], p[1], p[2] + aim]))
    };
    let mut picture =
        |seat: &mut Seat<LevelDemo>, e: Option<Eye>, name: &str| -> Result<(), String> {
            seat.eye = e;
            let frame = seat.picture(&gpu);
            seat.eye = None;
            shots.save(&frame?, name)
        };
    let p = place(&seat).ok_or("no figure")?;
    picture(
        &mut seat,
        Some(eye_beside(p, 3.2, 2.4, 2.6, 1.3)),
        "swim-stroke",
    )?;
    picture(&mut seat, None, "swim-behind")?;
    seat.game.key(Key::W, false);
    seat.game.key(Key::ShiftLeft, false);
    seat.run(&gpu, 4000)?;
    let p = place(&seat).ok_or("no figure")?;
    picture(
        &mut seat,
        Some(eye_beside(p, 1.4, 3.2, 2.0, 1.4)),
        "swim-tread",
    )?;
    Ok(shots.paths)
}

/// How far the view looks down at the treading arms (radians).
const LOOK_DOWN: f32 = 0.6;

/// Runs `text` in the map's server (the demo hosts it); its result.
fn on_server(game: &mut LevelDemo, text: &str) -> Result<String, String> {
    let server = game.server.as_mut().ok_or("no server")?;
    let vm = std::borrow::BorrowMut::<torque::app::server::GameServer>::borrow_mut(server).vm_mut();
    let out = vm.eval(text.as_bytes()).map_err(|e| format!("{e:?}"))?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Sets the own figure's arm action (`point`, `look`: its arms as the figure's animation has them).
fn arm(game: &mut LevelDemo, action: &str) -> Result<(), String> {
    on_server(
        game,
        &format!("ClientGroup.getObject(0).player.setArmThread({action});"),
    )
    .map(|_| ())
}

/// The valley's swimming pictures from the figure's eyes at `size` into `dir`: pointing on the meadow (`eye-point-land`), then
/// put down in the lake's deep water: treading (`eye-tread`, looking down at the arms, `eye-tread-down`, and pointing, `eye-point-tread`), three moments of the breaststroke (the hands coming forward, `eye-stroke`, meeting, `eye-stroke-2`,
/// and reaching out, `eye-stroke-3`),
/// pointing as it swims (`eye-point-swim`), diving below the surface (`eye-dive`), and a camera level with the surface, half
/// in the water (`eye-waterline`).
pub fn write_swim_eye(
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
        .position(|m| map_name(&m.path) == "valley")
        .ok_or("no valley map")?;
    seat.game.start_map(index)?;
    let mut waited = 0;
    while !(seat.game.in_world() && seat.game.figure().is_some()) && waited < START_LIMIT_MS {
        seat.run(&gpu, 100)?;
        waited += 100;
    }
    seat.run(&gpu, 2000)?;
    let level = seat
        .game
        .ground
        .water
        .as_ref()
        .map(|w| w.level)
        .ok_or("no lake")?;
    let mut picture =
        |seat: &mut Seat<LevelDemo>, e: Option<Eye>, name: &str| -> Result<(), String> {
            seat.eye = e;
            let frame = seat.picture(&gpu);
            seat.eye = None;
            shots.save(&frame?, name)
        };
    let place = |seat: &Seat<LevelDemo>| seat.game.figure().map(|f| f.position);
    // The way the figure walks from the spawn (toward the lake), from a second of it.
    let from = place(&seat).ok_or("no figure")?;
    seat.game.key(Key::W, true);
    seat.run(&gpu, 1000)?;
    seat.game.key(Key::W, false);
    seat.run(&gpu, 800)?;
    let to = place(&seat).ok_or("no figure")?;
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let len = dx.hypot(dy).max(1e-3);
    let ahead = [dx / len, dy / len];

    seat.game.set_first_person(true);
    arm(&mut seat.game, "point")?;
    seat.run(&gpu, 1000)?;
    picture(&mut seat, None, "eye-point-land")?;
    arm(&mut seat.game, "look")?;

    // Along that way to where the lake is 4 m deep, and 8 m on: put down there at the surface.
    let deep = (0..4000)
        .map(|i| {
            let d = i as f32 * 0.5;
            [to[0] + ahead[0] * d, to[1] + ahead[1] * d]
        })
        .find(|p| {
            seat.game
                .ground
                .height(p[0], p[1])
                .is_some_and(|h| level - h > 4.0)
        })
        .ok_or("no deep water on the way")?;
    let at = [deep[0] + ahead[0] * 8.0, deep[1] + ahead[1] * 8.0];
    on_server(
        &mut seat.game,
        &format!(
            "%p = ClientGroup.getObject(0).player; %p.setTransform(\"{} {} {}\" SPC getWords(%p.getTransform(), 3, 6));",
            at[0],
            at[1],
            level - 1.2
        ),
    )?;
    let mut waited = 0;
    while !seat.game.swimming() && waited < 5000 {
        seat.run(&gpu, 100)?;
        waited += 100;
    }
    if !seat.game.swimming() {
        return Err("the figure put in the lake does not swim".into());
    }
    seat.run(&gpu, 4000)?;
    picture(&mut seat, None, "eye-tread")?;
    // Looking down at the treading arms, and up again.
    seat.game.host.mouse_motion(0.0, LOOK_DOWN / 0.003);
    seat.run(&gpu, 300)?;
    picture(&mut seat, None, "eye-tread-down")?;
    seat.game.host.mouse_motion(0.0, -LOOK_DOWN / 0.003);
    seat.run(&gpu, 300)?;
    arm(&mut seat.game, "point")?;
    seat.run(&gpu, 1000)?;
    picture(&mut seat, None, "eye-point-tread")?;
    arm(&mut seat.game, "look")?;
    seat.run(&gpu, 500)?;

    seat.game.key(Key::W, true);
    seat.run(&gpu, 2160)?;
    picture(&mut seat, None, "eye-stroke")?;
    seat.run(&gpu, 160)?;
    picture(&mut seat, None, "eye-stroke-2")?;
    seat.run(&gpu, 320)?;
    picture(&mut seat, None, "eye-stroke-3")?;
    arm(&mut seat.game, "point")?;
    seat.run(&gpu, 1200)?;
    picture(&mut seat, None, "eye-point-swim")?;
    arm(&mut seat.game, "look")?;

    // Down (crouch dives) until the eye is a meter and a half under the surface.
    seat.game.key(Key::ControlLeft, true);
    let mut waited = 0;
    while seat.game.figure().is_some_and(|f| f.eye[2] > level - 1.5) && waited < 10_000 {
        seat.run(&gpu, 100)?;
        waited += 100;
    }
    seat.game.key(Key::ControlLeft, false);
    picture(&mut seat, None, "eye-dive")?;
    seat.game.key(Key::W, false);

    // A camera level with the surface, a little under it: the near plane straddles the waterline.
    let p = place(&seat).ok_or("no figure")?;
    let eye = [p[0] - ahead[0] * 3.0, p[1] - ahead[1] * 3.0, level - 0.02];
    let look = [eye[0] + ahead[0] * 10.0, eye[1] + ahead[1] * 10.0, eye[2]];
    picture(
        &mut seat,
        Some(crate::client::with_fov(look_at(eye, look))),
        "eye-waterline",
    )?;
    Ok(shots.paths)
}

/// The head's pitches the camera-switch pictures look at (radians, positive down): down, level and up.
pub const SWITCH_PITCHES: [(&str, f32); 3] = [("down", 0.6), ("level", 0.0), ("up", -0.45)];

/// The valley's camera-switch pictures at `size` into `dir`: the figure standing at the spawn with its head at each of
/// [`SWITCH_PITCHES`], seen from behind it (`switch-<pitch>-third`) and from its eyes (`switch-<pitch>-first`), so the two
/// views' directions can be compared.
pub fn write_switch(
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
        .position(|m| map_name(&m.path) == "valley")
        .ok_or("no valley map")?;
    seat.game.start_map(index)?;
    let mut waited = 0;
    while !(seat.game.in_world() && seat.game.figure().is_some()) && waited < START_LIMIT_MS {
        seat.run(&gpu, 100)?;
        waited += 100;
    }
    seat.run(&gpu, 2800)?;
    for (name, pitch) in SWITCH_PITCHES {
        let now = seat.game.figure().ok_or("no figure")?.pitch;
        let mouse = ((pitch - now) / 0.003).to_string();
        seat.game.call("look", &[mouse.as_str()])?;
        seat.game.set_first_person(false);
        seat.run(&gpu, 1500)?;
        let frame = seat.picture(&gpu)?;
        shots.save(&frame, &format!("switch-{name}-third"))?;
        seat.game.set_first_person(true);
        seat.run(&gpu, 100)?;
        let frame = seat.picture(&gpu)?;
        shots.save(&frame, &format!("switch-{name}-first"))?;
    }
    Ok(shots.paths)
}

/// The frames drawn of each forced correction (16 ms apart), the first before it.
#[cfg(feature = "smooth-prediction")]
const CORRECTION_FRAMES: usize = 12;

/// The plaza's forced corrections at `size` into `dir`: the server moves the standing figure 1.2 units sideways, and the frame
/// before and the frames after it are drawn one by one (`correction-<eye>-<predictor>-<nn>`, `00` the one before), from beside the figure (`side`) and from the game's
/// camera behind it (`own`), with TGE's predictor (`tge`) and the smoothed one (`smooth`). `correction-strip` puts the four
/// sequences in rows (side TGE's, side smoothed, own TGE's, own smoothed), the frames left to right.
#[cfg(feature = "smooth-prediction")]
pub fn write_correction(
    files: GameFiles,
    dir: &Path,
    size: (u32, u32),
) -> Result<Vec<PathBuf>, String> {
    let size = (size.0 / 4, size.1 / 4);
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let mut seat = Seat::new(LevelDemo::new(files)?, &gpu, size)?;
    let mut shots = Shots::new(dir)?;
    seat.run(&gpu, 200)?;
    let index = seat
        .game
        .maps
        .iter()
        .position(|m| map_name(&m.path) == "plaza")
        .ok_or("no plaza map")?;
    seat.game.start_map(index)?;
    let mut waited = 0;
    while !(seat.game.in_world() && seat.game.figure().is_some()) && waited < START_LIMIT_MS {
        seat.run(&gpu, 100)?;
        waited += 100;
    }
    seat.run(&gpu, 2000)?;
    let mut rows = Vec::new();
    for (eye, side) in [("side", true), ("own", false)] {
        for predictor in ["tge", "smooth"] {
            seat.game.call("setPredictor", &[predictor])?;
            seat.run(&gpu, 1500)?;
            let p = seat.game.figure().ok_or("no figure")?.position;
            seat.eye = side.then(|| {
                crate::client::with_fov(look_at(
                    [p[0] + 0.6, p[1] - 6.0, p[2] + 2.2],
                    [p[0] + 0.6, p[1], p[2] + 1.0],
                ))
            });
            let mut row = vec![seat.picture(&gpu)?];
            shots.save(&row[0], &format!("correction-{eye}-{predictor}-00"))?;
            on_server(
                &mut seat.game,
                "%p = ClientGroup.getObject(0).player; %t = %p.getTransform();                  %p.setTransform(VectorAdd(getWords(%t, 0, 2), \"1.2 0 0\") SPC getWords(%t, 3, 6));",
            )?;
            for i in 1..CORRECTION_FRAMES {
                seat.run(&gpu, torque::app::shots::FRAME_MS)?;
                let frame = seat.picture(&gpu)?;
                shots.save(&frame, &format!("correction-{eye}-{predictor}-{i:02}"))?;
                row.push(frame);
            }
            seat.eye = None;
            rows.push(row);
        }
    }
    shots.save(&strip(&rows), "correction-strip")?;
    Ok(shots.paths)
}

/// The frames of `rows` side by side, a row each.
#[cfg(feature = "smooth-prediction")]
fn strip(rows: &[Vec<Frame>]) -> Frame {
    let (w, h) = rows
        .first()
        .and_then(|r| r.first())
        .map_or((0, 0), |f| (f.width, f.height));
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0) as u32;
    let width = w * columns;
    let mut rgba = vec![0u8; (width * h * rows.len() as u32 * 4) as usize];
    for (r, row) in rows.iter().enumerate() {
        for (c, frame) in row.iter().enumerate() {
            for y in 0..h.min(frame.height) {
                let from = (y * frame.width * 4) as usize;
                let to = (((r as u32 * h + y) * width + c as u32 * w) * 4) as usize;
                let len = (w.min(frame.width) * 4) as usize;
                rgba[to..to + len].copy_from_slice(&frame.rgba[from..from + len]);
            }
        }
    }
    Frame {
        width,
        height: h * rows.len() as u32,
        rgba,
    }
}
