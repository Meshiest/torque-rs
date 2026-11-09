//! A model (`.glb`, `.gltf`, `.obj`, `.bbmodel`, `.dts`) into a `.dts` or a `.glb`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use torque_import::modelcore::Model;
use torque_import::{Options, bbmodel, caps, dts, gltf, obj};

fn usage() -> ExitCode {
    eprintln!(
        "usage: torque-convert <in.glb|in.gltf|in.obj|in.bbmodel|in.dts> <out.dts|out.glb> [--up y|z] [--scale N] [--rate N] \
         [--no-hull] [--force]"
    );
    ExitCode::FAILURE
}

pub fn ext(path: &str) -> String {
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
pub fn read(input: &str, options: &Options) -> Result<Model, String> {
    let bytes = read_capped(Path::new(input), caps::MAX_FILE_BYTES)?;
    let mut free = |_: u64| true;
    match ext(input).as_str() {
        "dts" => {
            let shape = torque_shapes::dts::parse(&bytes, &mut free).map_err(|e| e.to_string())?;
            dts::from_dts(&shape).map_err(|e| e.to_string())
        }
        "glb" | "gltf" | "obj" => {
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
            if ext(input) == "obj" {
                obj::read(&bytes, &mut side, options, &mut free).map_err(|e| e.to_string())
            } else {
                gltf::read(&bytes, &mut side, options, &mut free).map_err(|e| e.to_string())
            }
        }
        "bbmodel" => bbmodel::read(&bytes, options, &mut free).map_err(|e| e.to_string()),
        other => Err(format!("not a model file (.{other})")),
    }
}

/// The images the model carries, written beside `output` under their names. A name that two images share, or that a file in the
/// folder has already, is an error unless `force` (and nothing is written then): the user's other files are not overwritten.
pub fn write_images(model: &Model, output: &Path, force: bool) -> Result<usize, String> {
    let dir = output.parent().unwrap_or(Path::new(""));
    let mut planned: Vec<(PathBuf, &[u8])> = Vec::new();
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
        if !is_plain_file_name(&img.name) {
            return Err(format!(
                "image name {:?} is not a plain file name",
                img.name
            ));
        }
        let path = dir.join(format!("{}.{ext}", img.name));
        // Windows and macOS keep one file for names that differ only in case.
        let folded = path.to_string_lossy().to_lowercase();
        if !force
            && planned
                .iter()
                .any(|(p, _)| p.to_string_lossy().to_lowercase() == folded)
        {
            return Err(format!(
                "two images are named {} (--force writes the last one)",
                path.display()
            ));
        }
        planned.push((path, data));
    }
    for (path, _) in &planned {
        if !force && std::fs::symlink_metadata(path).is_ok() {
            return Err(format!("{} exists (--force overwrites it)", path.display()));
        }
    }
    for (path, data) in &planned {
        // `create_new` closes the gap between the check and the write.
        let mut options = std::fs::OpenOptions::new();
        options.write(true);
        if force {
            options.create(true).truncate(true);
        } else {
            options.create_new(true);
        }
        options
            .open(path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, data))
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(planned.len())
}

/// Whether `name` is safe as a file name on every platform: no folders, no drive or stream colon, no control or wildcard
/// characters, no leading dot, no trailing dot or space, and not a Windows device name (`CON`, `CONIN$`, `COM1`-`COM9` and
/// `LPT1`-`LPT9`, also with the superscript digits U+00B9, U+00B2 and U+00B3), with or without an extension. The same rules as
/// the script host's file names (a test holds the two together).
fn is_plain_file_name(name: &str) -> bool {
    const DEVICES: [&str; 6] = ["con", "prn", "aux", "nul", "conin$", "conout$"];
    if name.is_empty()
        || name.starts_with('.')
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    let stem = stem.to_lowercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix).is_some_and(|n| {
            let mut chars = n.chars();
            matches!(
                (chars.next(), chars.next()),
                (Some('0'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'), None)
            )
        })
    };
    !(DEVICES.contains(&stem.as_str()) || numbered("com") || numbered("lpt"))
}

/// The command for a model input (module docs).
pub fn run(args: &[String]) -> ExitCode {
    let mut plain = Vec::new();
    let mut options = Options::default();
    let mut force = false;
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
            "--force" => {
                force = true;
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
                let images = write_images(&model, Path::new(output), force)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use torque_import::modelcore::Image;

    /// A fresh folder in the build's own folder (a unit test has no `CARGO_TARGET_TMPDIR`).
    fn folder(name: &str) -> PathBuf {
        let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"),
            PathBuf::from,
        );
        let dir = target.join("convert-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn image(name: &str, data: &[u8]) -> Image {
        Image {
            name: name.into(),
            data: Some(data.to_vec()),
            mime: "image/png".into(),
            ..Image::default()
        }
    }

    fn model(images: Vec<Image>) -> Model {
        Model {
            images,
            ..Model::default()
        }
    }

    #[test]
    fn an_image_never_replaces_a_file_already_beside_the_output_unless_forced() {
        let dir = folder("exists");
        std::fs::write(dir.join("a.png"), b"mine").unwrap();
        let output = dir.join("out.dts");
        let m = model(vec![image("a", b"new"), image("fresh", b"new")]);
        let error = write_images(&m, &output, false).unwrap_err();
        assert!(
            error.contains("a.png") && error.contains("--force"),
            "{error}"
        );
        assert_eq!(std::fs::read(dir.join("a.png")).unwrap(), b"mine");
        assert!(
            !dir.join("fresh.png").exists(),
            "nothing is written when one is refused"
        );
        assert_eq!(write_images(&m, &output, true).unwrap(), 2);
        assert_eq!(std::fs::read(dir.join("a.png")).unwrap(), b"new");
        // A second run finds what the first wrote.
        let free = model(vec![image("fresh2", b"x")]);
        assert_eq!(write_images(&free, &output, false).unwrap(), 1);
        assert!(write_images(&free, &output, false).is_err());
    }

    #[test]
    fn the_name_rules_agree_with_the_script_hosts() {
        for name in [
            "bark",
            "leaf_2",
            "Wood.Floor",
            "console",
            "com10",
            "lpt",
            "a b",
            "CON",
            "con.x",
            "Nul.png",
            "aux .txt",
            "COM1",
            "com9",
            "lpt9",
            "LPT0",
            "COM\u{b9}",
            "lpt\u{b2}.txt",
            "Com\u{b3}",
            "CONIN$",
            "conout$.png",
            "tail.",
            "tail ",
            "a/b",
            "x:y",
            "q?",
            "",
            "t\u{1}",
        ] {
            // The script host also allows a leading dot, which an image name never has.
            let host = torque_ts_vm::host::sanitize_component(name.as_bytes());
            assert_eq!(is_plain_file_name(name), host, "{name:?}");
        }
    }

    #[test]
    fn two_images_of_one_name_are_refused_and_so_is_a_superscript_device_name() {
        let dir = folder("twice");
        let m = model(vec![image("a", b"1"), image("A", b"2")]);
        let error = write_images(&m, &dir.join("out.dts"), false).unwrap_err();
        assert!(error.contains("two images"), "{error}");
        assert!(!dir.join("a.png").exists());
        assert_eq!(write_images(&m, &dir.join("out.dts"), true).unwrap(), 2);
        let com = model(vec![image("COM\u{b9}", b"1")]);
        assert!(write_images(&com, &dir.join("out.dts"), true).is_err());
    }

    #[test]
    fn image_names_are_refused_when_a_platform_would_not_take_them_as_a_plain_file() {
        for ok in ["bark", "leaf_2", "Wood.Floor", "console", "com10", "lpt"] {
            assert!(is_plain_file_name(ok), "{ok}");
        }
        for bad in [
            "",
            ".hidden",
            "a/b",
            r"a\b",
            "c:x",
            "x:stream",
            "tail.",
            "tail ",
            "q?",
            "t\u{1}",
            "CON",
            "con",
            "Nul.png",
            "aux .txt",
            "COM1",
            "lpt9",
            "prn",
            "CONIN$",
            "conout$.png",
            "COM\u{b9}",
            "lpt\u{b2}.txt",
            "Com\u{b3}",
        ] {
            assert!(!is_plain_file_name(bad), "{bad:?}");
        }
    }
}
