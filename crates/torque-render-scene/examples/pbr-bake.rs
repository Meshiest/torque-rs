//! Bakes a material library for loading: each folder of `<library>` that holds a `.mat.toml` sidecar (one material) gets its
//! maps' chains made and block-compressed into KTX 2.0 files beside them (`hd::pbr::bake_files`: BC7 for color and ORM, BC5
//! for the normal, BC4 for the height, Zstandard levels), which the loader then takes instead of decoding the images. Prints
//! each map's format and what its levels take on the GPU against the RGBA8 chain they replace.
//!
//! Nothing is read outside a material's folder: a sidecar's paths are checked to stay inside it, and each file is read as a
//! plain file (no link followed) within 128 MiB.
//!
//! `cargo run -p torque-render-scene --release --example pbr-bake -- <library>`

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use images::ktx2::{self, Limits};
use torque_modelcore::pbr::Sidecar;
use torque_render_scene::hd::pbr::{self, retained_bytes};

/// The most bytes a map file may have.
const MAX_FILE_BYTES: u64 = 128 << 20;

fn read_capped(p: &Path) -> Option<Vec<u8>> {
    let (file, len) = torque_resource::plain::open_plain(p).ok()?;
    if len > MAX_FILE_BYTES {
        return None;
    }
    let mut bytes = Vec::with_capacity(len as usize);
    file.take(MAX_FILE_BYTES).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Writes `bytes` to `path`: a file or link already there is removed first (never written through), and
/// the new file is created exclusively.
fn write_fresh(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.is_dir() {
            return Err(std::io::Error::other("a folder is in the way"));
        }
        std::fs::remove_file(path).or_else(|_| std::fs::remove_dir(path))?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(bytes)
}

fn mib(b: u64) -> f64 {
    b as f64 / f64::from(1 << 20)
}

fn main() -> ExitCode {
    let Some(library) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: pbr-bake <library>");
        return ExitCode::FAILURE;
    };
    let mut folders: Vec<PathBuf> = match std::fs::read_dir(&library) {
        Ok(d) => d
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
        Err(e) => {
            eprintln!("{}: {e}", library.display());
            return ExitCode::FAILURE;
        }
    };
    folders.sort();
    let (mut rgba_total, mut gpu_total, mut failed) = (0u64, 0u64, 0usize);
    for folder in folders {
        let Some(sidecar) = std::fs::read_dir(&folder)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.to_string_lossy().ends_with(".mat.toml"))
        else {
            continue;
        };
        let name = folder
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let m = match read_capped(&sidecar)
            .and_then(|b| String::from_utf8(b).ok())
            .map(|t| Sidecar::parse(&t))
        {
            Some(Ok(Sidecar::One(m))) => *m,
            Some(Ok(Sidecar::Named(_))) => {
                eprintln!("{name}: the sidecar holds named materials, not one");
                failed += 1;
                continue;
            }
            Some(Err(e)) => {
                eprintln!("{name}: {e}");
                failed += 1;
                continue;
            }
            None => {
                eprintln!("{name}: the sidecar is not readable");
                failed += 1;
                continue;
            }
        };
        let start = Instant::now();
        let mut read = |rel: &str| read_capped(&folder.join(rel));
        let baked = match pbr::bake_files(&m, &mut read) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{name}: {e}");
                failed += 1;
                continue;
            }
        };
        println!("{name}: baked in {:.1} s", start.elapsed().as_secs_f64());
        for (path, b) in baked {
            let Ok(h) = ktx2::header(&b.file, &Limits::default()) else {
                continue;
            };
            let rgba = retained_bytes([(h.width, h.height)]);
            rgba_total += rgba;
            gpu_total += h.bytes;
            println!(
                "  {:<8} {:?} {} x {}, {} levels: {:.1} MiB on the GPU (RGBA8 {:.1} MiB), file {:.1} MiB",
                b.role.name(),
                h.format,
                h.width,
                h.height,
                h.level_count,
                mib(h.bytes),
                mib(rgba),
                mib(b.file.len() as u64)
            );
            if let Err(e) = write_fresh(&folder.join(&path), &b.file) {
                eprintln!("{name}: {path}: {e}");
                failed += 1;
            }
        }
    }
    println!(
        "the maps take {:.1} MiB on the GPU, {:.1} MiB less than RGBA8's {:.1} MiB",
        mib(gpu_total),
        mib(rgba_total.saturating_sub(gpu_total)),
        mib(rgba_total)
    );
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
