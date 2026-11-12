//! A showcase of the GPU particle pool's effects, drawn at the High look with the pool on: sparks, smoke, fireflies, a magic
//! circle, a portal, a railgun, fireworks, an explosion, a lightning storm, a fountain, volumetric fog, lava, petals, a galaxy
//! and digital rain. Each effect is datablocks
//! (`ParticleData` and `ParticleEmitterData` as the client holds them, with the engine's optional fields: turbulence,
//! sizeByVelocity, collide) and a script that moves their emitters each step; the glowing figures are meshes made each frame;
//! every texture and shape is made here.
//!
//! `cargo run -p torque-render-hd --release --example particles-showcase` opens a window with a tab for each show
//! (`app`). `-- --record <out> [--clip <name>]... [--size WxH] [--fps N] [--seconds S]` instead records them headless: each
//! runs from a still stage at a fixed step (two simulation steps a frame), draws the frames that let the picture and the
//! particles settle, then writes its seconds of frames at `--fps` as `<out>/<clip>/0000.png`, ... and prints its particles
//! alive and its median GPU time a frame with the passes that take it (the frames make videos with any encoder:
//! `ffmpeg -framerate 30 -i <out>/sparks/%04d.png ...`); `--bare` records the stages alone, the effects stepped but not drawn,
//! to diff an effect against (a glow must only add light). `-- --check` builds every show headless, draws a few frames of each
//! and checks its pool holds particles.

mod app;
mod effects;
mod font;
mod scenery;
mod show;
mod sprites;

use std::path::{Path, PathBuf};

use torque_render_core::Gpu;
use torque_render_hd::profile::Profile;

use crate::show::{SHOWS, Show};

enum Mode {
    Window,
    Check,
    Record(Record),
}

struct Record {
    out: PathBuf,
    clips: Vec<&'static str>,
    size: (u32, u32),
    fps: u32,
    seconds: Option<f32>,
    bare: bool,
}

fn args() -> Result<Mode, String> {
    let mut out = None;
    let mut record = false;
    let mut check = false;
    let mut r = Record {
        out: PathBuf::new(),
        clips: Vec::new(),
        size: (1280, 720),
        fps: 30,
        seconds: None,
        bare: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--record" => record = true,
            "--check" => check = true,
            "--bare" => r.bare = true,
            "--clip" => {
                let c = value()?;
                let name = SHOWS
                    .iter()
                    .find(|s| s.0 == c)
                    .map(|s| s.0)
                    .ok_or(format!("no clip {c}"))?;
                r.clips.push(name);
            }
            "--fps" => r.fps = value()?.parse().map_err(|e| format!("--fps: {e}"))?,
            "--seconds" => {
                r.seconds = Some(value()?.parse().map_err(|e| format!("--seconds: {e}"))?);
            }
            "--size" => {
                let v = value()?;
                let (w, h) = v.split_once('x').ok_or(format!("--size {v}: WxH"))?;
                r.size = (
                    w.parse().map_err(|e| format!("--size: {e}"))?,
                    h.parse().map_err(|e| format!("--size: {e}"))?,
                );
            }
            _ if out.is_none() && !arg.starts_with("--") => out = Some(PathBuf::from(arg)),
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    if check {
        return Ok(Mode::Check);
    }
    if !record {
        return Ok(Mode::Window);
    }
    r.out = out.ok_or("--record needs an output folder")?;
    if r.clips.is_empty() {
        r.clips = SHOWS.iter().map(|s| s.0).collect();
    }
    if r.fps == 0 || r.seconds.is_some_and(|s| s <= 0.0) {
        return Err("--fps and --seconds must be above 0".into());
    }
    Ok(Mode::Record(r))
}

fn write_png(path: &Path, rgba: &[u8], (w, h): (u32, u32)) -> Result<(), String> {
    let rgb: Vec<u8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_compression(png::Compression::Fast);
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(&rgb))
        .map_err(|e| e.to_string())
}

/// What a recorded clip measured.
struct Report {
    frames: usize,
    live_last: u32,
    live_most: u32,
    gpu_ms: f64,
    /// The GPU passes by their median ms, the costly ones first.
    passes: Vec<(&'static str, f64)>,
}

fn clip(gpu: &Gpu, r: &Record, name: &str, preroll: f32, seconds: f32) -> Result<Report, String> {
    let mut show = Show::new(gpu, name, r.size)?;
    show.bare = r.bare;
    let target = gpu
        .offscreen(r.size.0, r.size.1)
        .map_err(|e| e.to_string())?;
    let format = target.color.texture().format();
    let dir = r.out.join(name);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let settle = (preroll * r.fps as f32).round() as u64;
    let total = settle + (seconds * r.fps as f32).round() as u64;
    let mut profile = None;
    let mut report = Report {
        frames: 0,
        live_last: 0,
        live_most: 0,
        gpu_ms: 0.0,
        passes: Vec::new(),
    };
    for k in 0..total {
        show.frame(&Show::steps_at(k, r.fps), None, (&target.color, format))?;
        if k < settle {
            continue;
        }
        let p = profile.get_or_insert_with(|| Profile::begin(&mut show.world));
        p.frame(&mut show.world);
        let shot = gpu.read_back(&target).map_err(|e| e.to_string())?;
        write_png(
            &dir.join(format!("{:04}.png", k - settle)),
            &shot.rgba,
            r.size,
        )?;
        report.frames += 1;
        if (k - settle).is_multiple_of(30) || k + 1 == total {
            let live = show.live()?;
            report.live_last = live;
            report.live_most = report.live_most.max(live);
        }
    }
    if let Some(p) = profile {
        let (mut rows, total) = p.gpu_medians();
        report.gpu_ms = total;
        rows.sort_by(|a, b| b.1.total_cmp(&a.1));
        rows.retain(|r| r.1 >= 0.05);
        report.passes = rows;
    }
    Ok(report)
}

fn record(r: &Record) -> Result<(), String> {
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    for name in &r.clips {
        let (_, _, preroll, own) = SHOWS
            .iter()
            .copied()
            .find(|s| s.0 == *name)
            .expect("a known clip");
        let report = clip(&gpu, r, name, preroll, r.seconds.unwrap_or(own))?;
        let passes: Vec<String> = report
            .passes
            .iter()
            .map(|(n, ms)| format!("{n} {ms:.2}"))
            .collect();
        println!(
            "{name}: {} frames at {}x{}, {} particles alive at the end ({} at most), GPU {:.2} ms a frame ({})",
            report.frames,
            r.size.0,
            r.size.1,
            report.live_last,
            report.live_most,
            report.gpu_ms,
            passes.join(", ")
        );
    }
    Ok(())
}

/// Every show built and drawn for a second and a half, its pool then holding particles.
fn check() -> Result<(), String> {
    let gpu = Gpu::new().map_err(|e| e.to_string())?;
    let size = (640, 360);
    let target = gpu.offscreen(size.0, size.1).map_err(|e| e.to_string())?;
    let format = target.color.texture().format();
    for (name, ..) in SHOWS {
        let mut show = Show::new(&gpu, name, size)?;
        for k in 0..90 {
            show.frame(&Show::steps_at(k, 60), None, (&target.color, format))?;
        }
        let live = show.live()?;
        if live == 0 {
            return Err(format!(
                "{name}: the pool holds no particles after 90 frames"
            ));
        }
        println!("{name}: ok, {live} particles alive");
    }
    Ok(())
}

fn main() -> Result<(), String> {
    match args()? {
        Mode::Window => app::run(),
        Mode::Check => check(),
        Mode::Record(r) => record(&r),
    }
}
