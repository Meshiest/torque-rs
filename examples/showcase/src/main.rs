//! Pictures of the example games for the project's front page, each the world alone (no menu, HUD or hints) at the renderer's
//! highest quality, drawn offscreen, and the four set in a grid.
//!
//! `showcase <dir>` writes into `dir`: `car.png` (the vehicle demo's buggy in a turn of the race track, its dust and tire
//! tracks, the forests behind), `shot.png` (the arena: one player's bolt in flight toward another, the muzzle's flare; every
//! frame of the shot in `shot-frames/`), `valley.png` (the level demo's valley: the lake at golden hour) and `office.png` (the
//! level demo's office), each at the panel size (`--size WxH`, 1920x1080 by default); then `hero.png`, the four in a 2 x 2
//! grid of [`WIDTH`] x [`HEIGHT`] with thin transparent gutters (car and shot on top, valley and office below), and
//! `hero@2x.png` at twice that. `--examples` draws the examples' other pictures instead (no grid): `plaza.png` (the level
//! demo's plaza from above), `valley-wide.png` (the valley from high on its eastern slope) and `character.png` (the character
//! example's figure mid-stride). `--only <name>` draws that one picture alone (no grid); `--grid` sets the grid again from the
//! four pictures already in `dir`.

use std::path::{Path, PathBuf};

use torque::app::cli::{Args, unknown};
use torque::app::shots::write_png;
use torque::app::window::offscreen_gpu;
use torque_render_core::{Frame, Gpu};

/// The grid's size, and its gutters' width (the 2x grid's are twice these).
const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const GUTTER: u32 = 8;
/// The panels' size by default.
const PANEL: (u32, u32) = (1920, 1080);

/// The panels in the grid's order: top left, top right, bottom left, bottom right.
const PANELS: [&str; 4] = ["car", "shot", "valley", "office"];
/// The examples' other pictures (`--examples`).
const EXTRAS: [&str; 3] = ["plaza", "valley-wide", "character"];

fn draw(gpu: &Gpu, name: &str, size: (u32, u32), dir: &Path) -> Result<Frame, String> {
    match name {
        "car" => torque_example_vehicle_demo::showcase::turn_picture(
            gpu,
            size,
            torque_example_vehicle_demo::files::GameFiles::bundled,
        ),
        "shot" => {
            let frames = torque_example_fps::showcase::shot_frames(
                gpu,
                size,
                torque_example_fps::files::GameFiles::bundled,
            )?;
            let folder = dir.join("shot-frames");
            std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
            for (k, f) in frames.iter().enumerate() {
                write_png(f, &folder.join(format!("{k:02}.png"))).map_err(|e| e.to_string())?;
            }
            let k = torque_example_fps::showcase::PICTURE_FRAME.min(frames.len() - 1);
            frames
                .into_iter()
                .nth(k)
                .ok_or_else(|| "no frames".to_string())
        }
        "valley" | "office" => {
            let files = torque_example_level_demo::files::GameFiles::bundled()
                .map_err(|e| e.to_string())?;
            let picture = if name == "valley" {
                torque_example_level_demo::showcase::valley_picture
            } else {
                torque_example_level_demo::showcase::office_picture
            };
            picture(gpu, files, size)
        }
        "plaza" | "valley-wide" => {
            use torque_example_level_demo::showcase::{VALLEY_VIEWS, seat_on, valley_eye, view};
            let files = torque_example_level_demo::files::GameFiles::bundled()
                .map_err(|e| e.to_string())?;
            if name == "plaza" {
                let mut seat = seat_on(gpu, files, size, "plaza")?;
                seat.eye = Some(seat.game.overview_eye());
                seat.world_picture(gpu)
            } else {
                let mut seat = seat_on(gpu, files, size, "valley")?;
                let (_, from, to) = VALLEY_VIEWS[1];
                view(gpu, &mut seat, valley_eye(from), to)
            }
        }
        "character" => {
            use torque_example_character::{game::CharacterGame, shots};
            let mut game = CharacterGame::new()?;
            shots::picture_camera(&mut game);
            let mut view = game.view(gpu);
            view.set_quality(torque_render_scene::hd::Quality::Ultra);
            let shots = shots::play(&mut game, gpu, &mut view, size, shots::WORLD_STATES)?;
            shots
                .into_iter()
                .find(|(n, _)| *n == "walk")
                .map(|(_, f)| f)
                .ok_or_else(|| "no walk picture".to_string())
        }
        _ => Err(format!("no picture named {name}")),
    }
}

/// `frame` resampled to `w` x `h` (each output pixel the mean of a grid of samples of its footprint, bilinear between source
/// pixels), covering it: scaled to the larger of the two ratios and cropped about the middle.
fn fit(frame: &Frame, w: u32, h: u32) -> Frame {
    let scale = (w as f32 / frame.width as f32).max(h as f32 / frame.height as f32);
    let (sw, sh) = (frame.width as f32 * scale, frame.height as f32 * scale);
    let (ox, oy) = ((sw - w as f32) / 2.0, (sh - h as f32) / 2.0);
    const N: u32 = 4;
    let at = |x: f32, y: f32, c: usize| -> f32 {
        let x = x.clamp(0.0, (frame.width - 1) as f32);
        let y = y.clamp(0.0, (frame.height - 1) as f32);
        let (x0, y0) = (x.floor() as u32, y.floor() as u32);
        let (x1, y1) = (
            (x0 + 1).min(frame.width - 1),
            (y0 + 1).min(frame.height - 1),
        );
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let p = |x: u32, y: u32| f32::from(frame.rgba[(4 * (y * frame.width + x)) as usize + c]);
        let top = p(x0, y0) + (p(x1, y0) - p(x0, y0)) * tx;
        let bottom = p(x0, y1) + (p(x1, y1) - p(x0, y1)) * tx;
        top + (bottom - top) * ty
    };
    let mut rgba = vec![0u8; (4 * w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0.0f32; 3];
            for j in 0..N {
                for i in 0..N {
                    let sx = (x as f32 + (i as f32 + 0.5) / N as f32 + ox) / scale - 0.5;
                    let sy = (y as f32 + (j as f32 + 0.5) / N as f32 + oy) / scale - 0.5;
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += at(sx, sy, c);
                    }
                }
            }
            let o = (4 * (y * w + x)) as usize;
            for c in 0..3 {
                rgba[o + c] = (sum[c] / (N * N) as f32).round() as u8;
            }
            rgba[o + 3] = 255;
        }
    }
    Frame {
        width: w,
        height: h,
        rgba,
    }
}

/// The four `panels` in a 2 x 2 grid `w` x `h` with `gutter`-wide transparent gutters between them (none round the edge).
fn grid(panels: &[Frame; 4], w: u32, h: u32, gutter: u32) -> Frame {
    let (pw, ph) = ((w - gutter) / 2, (h - gutter) / 2);
    let mut rgba = vec![0u8; (4 * w * h) as usize];
    for (k, panel) in panels.iter().enumerate() {
        let p = fit(panel, pw, ph);
        let x0 = if k % 2 == 0 { 0 } else { w - pw };
        let y0 = if k / 2 == 0 { 0 } else { h - ph };
        for y in 0..ph {
            let from = (4 * y * pw) as usize;
            let to = (4 * ((y0 + y) * w + x0)) as usize;
            rgba[to..to + 4 * pw as usize].copy_from_slice(&p.rgba[from..from + 4 * pw as usize]);
        }
    }
    Frame {
        width: w,
        height: h,
        rgba,
    }
}

/// Writes `frame` as a PNG with its alpha (the gutters see-through).
fn write_rgba(frame: &Frame, path: &Path) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer
        .write_image_data(&frame.rgba)
        .map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())
}

fn read_png(path: &Path) -> Result<Frame, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width, info.height);
    let channels = buf.len() / (w * h) as usize;
    let rgba = buf
        .chunks_exact(channels)
        .take((w * h) as usize)
        .flat_map(|p| [p[0], p[1], p[2], if channels == 4 { p[3] } else { 255 }])
        .collect();
    Ok(Frame {
        width: w,
        height: h,
        rgba,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Args::from_env();
    let mut dir: Option<PathBuf> = None;
    let mut size = PANEL;
    let mut only = None;
    let mut grid_only = false;
    let mut examples = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--size" => {
                let v = args.value(&arg, "a size WxH")?;
                let (w, h) = v.split_once('x').ok_or("--size takes WxH")?;
                size = (w.parse()?, h.parse()?);
            }
            "--only" => only = Some(args.value(&arg, "a picture's name")?),
            "--grid" => grid_only = true,
            "--examples" => examples = true,
            _ if !arg.starts_with('-') && dir.is_none() => dir = Some(PathBuf::from(arg)),
            _ => return Err(unknown(&arg).into()),
        }
    }
    let dir =
        dir.ok_or("usage: showcase <dir> [--size WxH] [--only <name>] [--examples] [--grid]")?;
    std::fs::create_dir_all(&dir)?;
    if !grid_only {
        let gpu = offscreen_gpu()?;
        let names: Vec<&str> = match &only {
            Some(o) => vec![o.as_str()],
            None if examples => EXTRAS.to_vec(),
            None => PANELS.to_vec(),
        };
        for name in names {
            let frame = draw(&gpu, name, size, &dir)?;
            let path = dir.join(format!("{name}.png"));
            write_png(&frame, &path)?;
            println!("{}", path.display());
        }
        if only.is_some() || examples {
            return Ok(());
        }
    }
    let panels = PANELS.map(|n| read_png(&dir.join(format!("{n}.png"))));
    let [a, b, c, d] = panels;
    let panels = [a?, b?, c?, d?];
    for (name, scale) in [("hero.png", 1), ("hero@2x.png", 2)] {
        let frame = grid(&panels, WIDTH * scale, HEIGHT * scale, GUTTER * scale);
        let path = dir.join(name);
        write_rgba(&frame, &path)?;
        println!("{}", path.display());
    }
    Ok(())
}
