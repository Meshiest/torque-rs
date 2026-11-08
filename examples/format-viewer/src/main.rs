//! The format viewer: pictures of Source content drawn offscreen (crate docs in `Cargo.toml`).
//!
//! `TORQUE_SOURCE_GAME_DIR` names one or more game folders (each with a `gameinfo.txt`), separated by `;`. Commands:
//!
//! - `list <prefix> <suffix>`: the mounted paths that start and end so.
//! - `cat <path>`: a mounted file's bytes to the standard output.
//! - `sheet <out.png> <material or texture>...`: the textures of materials (or `.vtf` files) side by side.
//! - `model <out-dir> <model.mdl> [sequence] [frames]`: a model turned about (or one sequence's frames).
//! - `map <out-dir> <map.bsp> [shots]`: a map's geometry, displacements and static props from a few cameras.
//! - `effect <out-dir> <file.pcf> [system]`: a particle system's frames (or the file's systems listed).
//! - `sound <out.wav> <entry or wave> [seconds]`: a soundscript entry or a wave mixed offline into a file.

mod audio;
mod draw;
mod effect;
mod map;
mod model;

use std::path::{Path, PathBuf};
use std::rc::Rc;

use torque_engine::EngineBuilder;
use torque_engine::assets::Assets;
use torque_format_valve::files::SourceFiles;
use torque_format_valve::vmt::{Vmt, material_path};
use torque_format_valve::{Texture, ValveFormatPlugins};

/// The game's files and the asset cache that reads them.
pub struct Content {
    pub files: SourceFiles,
    pub assets: Assets,
}

impl Content {
    /// The folders `TORQUE_SOURCE_GAME_DIR` names, mounted.
    fn mount() -> Result<Content, String> {
        let dirs = std::env::var("TORQUE_SOURCE_GAME_DIR").map_err(|_| {
            "set TORQUE_SOURCE_GAME_DIR to a game folder holding gameinfo.txt".to_string()
        })?;
        let mut files = SourceFiles::new();
        for d in dirs.split(';').filter(|d| !d.trim().is_empty()) {
            files
                .add_gameinfo(Path::new(d.trim()))
                .map_err(|e| e.to_string())?;
        }
        for s in &files.skipped {
            eprintln!("not mounted: {s}");
        }
        let engine = EngineBuilder::new()
            .add_plugins(ValveFormatPlugins)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Content {
            files,
            assets: Assets::new(&engine),
        })
    }

    /// A material by name (`concrete/wall01` or a path), its `patch` applied.
    pub fn material(&self, name: &str) -> Option<Rc<Vmt>> {
        let path = material_path(name);
        if let Some(m) = self.assets.get::<Vmt>(&format!("{path}#patched")) {
            return Some(m);
        }
        let m = self.assets.load::<Vmt>(&self.files, &path).ok()?;
        let m = if m.is_patch() {
            Rc::new(
                (*m).clone()
                    .patched(&mut |inc| {
                        self.assets
                            .load::<Vmt>(&self.files, &material_path(inc))
                            .ok()
                            .map(|m| (*m).clone())
                    })
                    .ok()?,
            )
        } else {
            m
        };
        Some(m)
    }

    /// A texture by its path.
    pub fn texture(&self, path: &str) -> Option<Rc<Texture>> {
        match self.assets.load::<Texture>(&self.files, path) {
            Ok(t) => Some(t),
            Err(e) => {
                eprintln!("texture {path}: {e}");
                None
            }
        }
    }
}

/// Writes an RGB or RGBA picture.
pub fn write_png(path: &Path, w: u32, h: u32, rgba: bool, data: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(if rgba {
        png::ColorType::Rgba
    } else {
        png::ColorType::Rgb
    });
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(data))
        .map_err(|e| e.to_string())
}

/// The textures of `names` (materials or `.vtf` paths) in a grid of 256-pixel cells, each sampled from its top level.
fn sheet(c: &Content, out: &Path, names: &[String]) -> Result<(), String> {
    const CELL: u32 = 256;
    let cols = (names.len() as f32).sqrt().ceil().max(1.0) as u32;
    let rows = (names.len() as u32).div_ceil(cols).max(1);
    let (w, h) = (cols * CELL, rows * CELL);
    let mut img = vec![32u8; (w * h * 3) as usize];
    for (i, name) in names.iter().enumerate() {
        let path = if name.ends_with(".vtf") {
            name.clone()
        } else {
            let Some(m) = c.material(name) else {
                eprintln!("no material {name}");
                continue;
            };
            let Some(t) = m.base_texture() else {
                eprintln!("{name}: no $basetexture ({})", m.shader);
                continue;
            };
            t
        };
        let Some(t) = c.texture(&path) else { continue };
        let l = &t.levels[0];
        println!(
            "{name}: {} x {} {:?}, {} levels",
            t.vtf.width,
            t.vtf.height,
            t.vtf.format,
            t.levels.len()
        );
        let (ox, oy) = ((i as u32 % cols) * CELL, (i as u32 / cols) * CELL);
        for y in 0..CELL {
            for x in 0..CELL {
                let sx = (x * l.width / CELL).min(l.width - 1);
                let sy = (y * l.height / CELL).min(l.height - 1);
                let s = ((sy * l.width + sx) * 4) as usize;
                let o = (((oy + y) * w + ox + x) * 3) as usize;
                img[o..o + 3].copy_from_slice(&l.rgba[s..s + 3]);
            }
        }
    }
    write_png(out, w, h, false, &img)
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        return Err("usage: list | sheet | model | map (see the source's docs)".into());
    };
    let mut c = Content::mount()?;
    match cmd.as_str() {
        "list" => {
            let prefix = args.get(1).map_or("", String::as_str);
            let suffix = args.get(2).map_or("", String::as_str);
            for p in c.files.list(prefix, suffix) {
                println!("{p}");
            }
            Ok(())
        }
        "cat" => {
            let path = args.get(1).ok_or("cat <path>")?;
            let b = c
                .files
                .read_path(path)
                .ok_or_else(|| format!("no {path}"))?
                .map_err(|e| e.to_string())?;
            use std::io::Write;
            std::io::stdout().write_all(&b).map_err(|e| e.to_string())?;
            Ok(())
        }
        "sheet" => {
            let out = PathBuf::from(args.get(1).ok_or("sheet <out.png> <names>...")?);
            sheet(&c, &out, &args[2..])
        }
        "model" => model::run(&c, &args[1..]),
        "map" => map::run(&mut c, &args[1..]),
        "effect" => effect::run(&c, &args[1..]),
        "sound" => audio::run(&c, &args[1..]),
        "sun-check" => {
            // How the map's lightmaps agree with its sun, read in each orientation (a check of the lightmap mapping).
            let path = args.get(1).ok_or("sun-check <maps/x.bsp> [luxels]")?;
            let n = args
                .get(2)
                .and_then(|v| v.parse().ok())
                .unwrap_or(40_000usize);
            let map: std::rc::Rc<torque_format_valve::bsp::Bsp> = c
                .assets
                .load(&c.files, path)
                .map_err(|e| format!("{path}: {e}"))?;
            let a = map
                .sun_agreement(n)
                .map_err(|e| e.to_string())?
                .ok_or("the map has no sky light")?;
            for (o, (t, fs, ft)) in torque_format_valve::bsp::ORIENTATIONS.iter().enumerate() {
                let pct = |(a, n): (u64, u64)| {
                    if n == 0 {
                        "-".to_string()
                    } else {
                        format!("{:.1} % of {n}", 100.0 * a as f64 / n as f64)
                    }
                };
                println!(
                    "transposed {t:5} flip s {fs:5} flip t {ft:5}: faces {}, displacements {}",
                    pct(a.faces[o]),
                    pct(a.displacements[o])
                );
            }
            Ok(())
        }
        other => Err(format!("unknown command {other}")),
    }
}
