//! A Quake or Valve 220 `.map` into a `.dif` and a `.mis` fragment.
//!
//! ```text
//! torque-convert <in.map> <out.dif> [--scale <map units a unit, 32>] [--lightmap-texel <units, 0.5>]
//!     [--textures <folder>] [--mission <out.mis>] [--name <object name>] [--file <interiorFile path>]
//!     [--position "x y z"] [--rotation "ax ay az degrees"] [--instance-scale "x y z"]
//! ```
//!
//! The textures' sizes come from the PNG or JPEG of each name in `--textures` (the folder listed once, each texture's header
//! read once; a missing one is taken as 256 x 256 with a warning). The fragment holds the `InteriorInstance` (its
//! `interiorFile` the `--file` path, else the output's file name) and the map's mission objects, placed with it; it goes to `--mission`, else beside the `.dif` with the `.mis` extension.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use torque_interior_builder::map::{self, MapOptions};
use torque_interior_builder::mission::{Placement, fragment};
use torque_interior_builder::{Options, build};
use torque_resource::plain::read_plain;

fn usage() -> ExitCode {
    eprintln!(
        "usage: torque-convert <in.map> <out.dif> [--scale N] [--lightmap-texel T] [--textures DIR] [--mission OUT.mis] \
         [--name NAME] [--file PATH] [--position \"x y z\"] [--rotation \"ax ay az deg\"] [--instance-scale \"x y z\"]"
    );
    ExitCode::FAILURE
}

/// The width and height a PNG's or a JPEG's header gives.
fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let be32 = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) && bytes.get(12..16)? == b"IHDR" {
        return Some((be32(16)?, be32(20)?));
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        let mut at = 2usize;
        while at + 9 < bytes.len() {
            if bytes[at] != 0xff {
                return None;
            }
            let marker = bytes[at + 1];
            let len = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
            if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
                let h = u16::from_be_bytes([bytes[at + 5], bytes[at + 6]]);
                let w = u16::from_be_bytes([bytes[at + 7], bytes[at + 8]]);
                return Some((u32::from(w), u32::from(h)));
            }
            at += 2 + len;
        }
    }
    None
}

/// The most image files of the textures folder looked at.
const MAX_TEXTURE_FILES: usize = 65_536;

/// The textures folder's PNG and JPEG files by their name without the extension, lower case (read once).
fn texture_files(dir: &Path) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten().take(MAX_TEXTURE_FILES) {
        let file = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if let Some((stem, ext)) = file.rsplit_once('.')
            && ["png", "jpg", "jpeg"].contains(&ext)
        {
            out.entry(stem.to_string()).or_insert_with(|| entry.path());
        }
    }
    out
}

/// The size of texture `name` (its last path part, any case) among `files`, from the file's first 64 KiB.
fn texture_size(files: &HashMap<String, PathBuf>, name: &str) -> Option<(u32, u32)> {
    let base = name.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
    let path = files.get(&base)?;
    let mut head = vec![0u8; 64 << 10];
    let n = std::io::Read::read(&mut std::fs::File::open(path).ok()?, &mut head).ok()?;
    image_size(&head[..n])
}

fn floats<const N: usize>(s: &str) -> Option<[f64; N]> {
    let v: Vec<f64> = s
        .split_whitespace()
        .map(|t| t.parse().ok())
        .collect::<Option<_>>()?;
    v.try_into()
        .ok()
        .filter(|a: &[f64; N]| a.iter().all(|x| x.is_finite()))
}

/// The command for a `.map` input (module docs).
pub fn run(args: &[String]) -> ExitCode {
    let mut plain = Vec::new();
    let mut scale = 32.0;
    let mut options = Options::default();
    let mut textures: Option<PathBuf> = None;
    let mut mission: Option<PathBuf> = None;
    let mut name = String::new();
    let mut file: Option<String> = None;
    let mut at = Placement::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned();
        let ok = match a.as_str() {
            "--scale" => value()
                .and_then(|v| v.parse().ok())
                .map(|v| scale = v)
                .is_some(),
            "--lightmap-texel" => value()
                .and_then(|v| v.parse().ok())
                .map(|v| options.lightmap_texel = v)
                .is_some(),
            "--textures" => value().map(|v| textures = Some(v.into())).is_some(),
            "--mission" => value().map(|v| mission = Some(v.into())).is_some(),
            "--name" => value().map(|v| name = v).is_some(),
            "--file" => value().map(|v| file = Some(v)).is_some(),
            "--position" => value()
                .and_then(|v| floats::<3>(&v))
                .map(|v| at.position = v.into())
                .is_some(),
            "--rotation" => value()
                .and_then(|v| floats::<4>(&v))
                .map(|v| {
                    at.axis = [v[0], v[1], v[2]].into();
                    at.angle = v[3];
                })
                .is_some(),
            "--instance-scale" => value()
                .and_then(|v| floats::<3>(&v))
                .map(|v| at.scale = v.into())
                .is_some(),
            s if s.starts_with("--") => false,
            _ => {
                plain.push(a.clone());
                true
            }
        };
        if !ok {
            eprintln!("bad or missing value for {a}");
            return usage();
        }
    }
    let [input, output] = plain.as_slice() else {
        return usage();
    };
    let text = match read_plain(Path::new(input), map::MAX_MAP_BYTES as u64, None) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) => {
            eprintln!("{input}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let files = textures.as_deref().map(texture_files).unwrap_or_default();
    let sizes = |n: &str| texture_size(&files, n);
    let parsed = match map::parse(
        &text,
        &MapOptions {
            units_per_meter: scale,
            texture_size: &sizes,
        },
    ) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{input}: {e}");
            return ExitCode::FAILURE;
        }
    };
    for w in &parsed.warnings {
        eprintln!("warning: {w}");
    }
    let built = match build(&parsed.brushes, &options).and_then(|b| b.bytes().map(|x| (b, x))) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{input}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let (built, bytes) = built;
    if let Err(e) = std::fs::write(output, &bytes) {
        eprintln!("{output}: {e}");
        return ExitCode::FAILURE;
    }
    let out = Path::new(output);
    let mis = mission.unwrap_or_else(|| out.with_extension("mis"));
    let file = file.unwrap_or_else(|| {
        out.file_name()
            .map_or_else(|| output.clone(), |f| f.to_string_lossy().into_owned())
    });
    if let Err(e) = std::fs::write(&mis, fragment(&name, &file, &at, &parsed.objects)) {
        eprintln!("{}: {e}", mis.display());
        return ExitCode::FAILURE;
    }
    let i = &built.dif.details[0];
    println!(
        "{output}: {} bytes; {} brushes, {} surfaces, {} null surfaces, {} BSP nodes, {} lightmap pages; {} mission objects in {}",
        bytes.len(),
        i.hulls.hulls.len(),
        i.surfaces.len(),
        i.null_surfaces.len(),
        i.bsp_nodes.len(),
        i.lightmaps.len(),
        parsed.objects.len(),
        mis.display()
    );
    ExitCode::SUCCESS
}
