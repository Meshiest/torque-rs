//! A model (`.glb`, `.gltf`, `.dts`) into a `.dts` or a `.glb`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use torque_import::modelcore::Model;
use torque_import::{Options, caps, dts, gltf};

fn usage() -> ExitCode {
    eprintln!(
        "usage: torque-convert <in.glb|in.gltf|in.dts> <out.dts|out.glb> [--up y|z] [--scale N] [--rate N] [--no-hull]"
    );
    ExitCode::FAILURE
}

fn ext(path: &str) -> String {
    Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// A file of at most `cap` bytes.
fn read_capped(path: &Path, cap: usize) -> Result<Vec<u8>, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > cap as u64 {
        return Err(format!("over {cap} bytes"));
    }
    std::fs::read(path).map_err(|e| e.to_string())
}

/// The model in the file `input`, its side files read from its folder.
fn read(input: &str, options: &Options) -> Result<Model, String> {
    let bytes = read_capped(Path::new(input), caps::MAX_FILE_BYTES)?;
    let mut free = |_: u64| true;
    match ext(input).as_str() {
        "dts" => {
            let shape = torque_shapes::dts::parse(&bytes, &mut free).map_err(|e| e.to_string())?;
            dts::from_dts(&shape).map_err(|e| e.to_string())
        }
        "glb" | "gltf" => {
            let dir = Path::new(input)
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            let dir = if dir.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                dir
            };
            // Side files under the input's folder, each read only as far as its buffer's declared length.
            let mut side = gltf::side_files(&dir);
            gltf::read(&bytes, &mut side, options, &mut free).map_err(|e| e.to_string())
        }
        other => Err(format!("not a model file (.{other})")),
    }
}

/// The images the model carries, written beside `output` under their names.
fn write_images(model: &Model, output: &Path) -> Result<usize, String> {
    let dir = output.parent().unwrap_or(Path::new(""));
    let mut written = 0;
    for img in &model.images {
        let Some(data) = &img.data else {
            continue;
        };
        let ext = if img.mime.contains("jpeg") {
            "jpg"
        } else {
            "png"
        };
        // The name is a file name of the reader's making (no folder), but it is checked again here.
        if img.name.is_empty() || img.name.contains(['/', '\\', ':']) || img.name.starts_with('.') {
            return Err(format!(
                "image name {:?} is not a plain file name",
                img.name
            ));
        }
        let path = dir.join(format!("{}.{ext}", img.name));
        std::fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))?;
        written += 1;
    }
    Ok(written)
}

/// The command for a model input (module docs).
pub fn run(args: &[String]) -> ExitCode {
    let mut plain = Vec::new();
    let mut options = Options::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned();
        let ok = match a.as_str() {
            "--up" => value()
                .filter(|v| v == "y" || v == "z")
                .map(|v| options.up = if v == "z" { 'z' } else { 'y' })
                .is_some(),
            "--scale" => value()
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|v| v.is_finite() && *v > 0.0)
                .map(|v| options.scale = v)
                .is_some(),
            "--rate" => value()
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|v| v.is_finite() && *v > 0.0 && *v <= 1000.0)
                .map(|v| options.rate = v)
                .is_some(),
            "--no-hull" => {
                options.box_hull = false;
                true
            }
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
    let model = match read(input, &options) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{input}: {e}");
            return ExitCode::FAILURE;
        }
    };
    for n in &model.notes {
        eprintln!("note: {n}");
    }
    let written = match ext(output).as_str() {
        "dts" => dts::to_dts(&model)
            .map(|s| torque_shapes::write::write_dts(&s))
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                let images = write_images(&model, Path::new(output))?;
                Ok((bytes, images))
            }),
        "glb" => gltf::write_glb(&model)
            .map(|b| (b, 0))
            .map_err(|e| e.to_string()),
        other => Err(format!("cannot write .{other}: give a .dts or a .glb")),
    };
    let (bytes, images) = match written {
        Ok(w) => w,
        Err(e) => {
            eprintln!("{output}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = std::fs::write(output, &bytes) {
        eprintln!("{output}: {e}");
        return ExitCode::FAILURE;
    }
    println!(
        "{output}: {} bytes; {} nodes, {} meshes, {} vertices, {} materials, {} sequences; {images} images written beside it",
        bytes.len(),
        model.nodes.len(),
        model.meshes.len(),
        model
            .meshes
            .iter()
            .map(|m| m.positions.len())
            .sum::<usize>(),
        model.materials.len(),
        model.animations.len(),
    );
    ExitCode::SUCCESS
}
