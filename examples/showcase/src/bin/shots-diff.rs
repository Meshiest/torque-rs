//! Compares two folders of pictures: `shots-diff [--ignore-corner WxH] [--skip <path>]... <a> <b>`.
//!
//! Every PNG under either folder is matched by its path below the folder. For each one the line says whether the sizes match,
//! how many pixels differ (any channel) and the largest difference in any channel. A picture only one folder has counts as a
//! difference. `--ignore-corner WxH` leaves the top right corner of that size out of every comparison (the corner readout of the
//! frame rate, which is measured on the clock and so differs from run to run). `--skip <path>` leaves out the picture at that path below the folders (named in the output, never silently). The exit code is 1 when any picture differs, 2 for a usage or read error, else 0.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

struct Picture {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn read(path: &Path) -> Result<Picture, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("picture too large")?];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let pixels = (info.width as usize) * (info.height as usize);
    let channels = info.color_type.samples();
    let rgba = buf
        .chunks_exact(channels)
        .take(pixels)
        .flat_map(|p| match channels {
            1 => [p[0], p[0], p[0], 255],
            2 => [p[0], p[0], p[0], p[1]],
            3 => [p[0], p[1], p[2], 255],
            _ => [p[0], p[1], p[2], p[3]],
        })
        .collect();
    Ok(Picture {
        width: info.width,
        height: info.height,
        rgba,
    })
}

/// Every `.png` below `root`, as paths relative to it.
fn pngs(root: &Path, at: &Path, found: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(at).map_err(|e| format!("{}: {e}", at.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            pngs(root, &path, found)?;
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("png"))
        {
            found.push(
                path.strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_path_buf(),
            );
        }
    }
    Ok(())
}

fn run(a: &Path, b: &Path, corner: (u32, u32), skip: &[String]) -> Result<bool, String> {
    let (mut in_a, mut in_b) = (Vec::new(), Vec::new());
    pngs(a, a, &mut in_a)?;
    pngs(b, b, &mut in_b)?;
    let mut names: Vec<PathBuf> = in_a.iter().chain(&in_b).cloned().collect();
    names.sort();
    names.dedup();
    let shown_names: Vec<String> = names
        .iter()
        .map(|n| n.to_string_lossy().replace('\\', "/"))
        .collect();
    if let Some(unmatched) = skip.iter().find(|s| !shown_names.contains(s)) {
        return Err(format!("--skip {unmatched} matches no picture"));
    }
    let mut differing = 0usize;
    let mut compared = 0usize;
    for name in &names {
        let shown = name.to_string_lossy().replace('\\', "/");
        if skip.contains(&shown) {
            println!("{shown}: skipped");
            continue;
        }
        if !in_a.contains(name) || !in_b.contains(name) {
            let only = if in_a.contains(name) {
                "first"
            } else {
                "second"
            };
            println!("{shown}: only in the {only} folder");
            differing += 1;
            continue;
        }
        let (pa, pb) = (read(&a.join(name))?, read(&b.join(name))?);
        compared += 1;
        if (pa.width, pa.height) != (pb.width, pb.height) {
            println!(
                "{shown}: size {}x{} against {}x{}",
                pa.width, pa.height, pb.width, pb.height
            );
            differing += 1;
            continue;
        }
        let mut pixels = 0usize;
        let mut max = 0u8;
        let (mut lo, mut hi) = ((u32::MAX, u32::MAX), (0u32, 0u32));
        let pairs = pa
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(pb.rgba.as_chunks::<4>().0);
        for (i, (x, y)) in pairs.enumerate() {
            let delta = x
                .iter()
                .zip(y)
                .map(|(p, q)| p.abs_diff(*q))
                .max()
                .unwrap_or(0);
            let (px, py) = (i as u32 % pa.width, i as u32 / pa.width);
            if delta > 0 && !(px + corner.0 >= pa.width && py < corner.1) {
                pixels += 1;
                max = max.max(delta);
                lo = (lo.0.min(px), lo.1.min(py));
                hi = (hi.0.max(px), hi.1.max(py));
            }
        }
        let bounds = if pixels > 0 {
            format!(", within ({},{})-({},{})", lo.0, lo.1, hi.0, hi.1)
        } else {
            String::new()
        };
        println!(
            "{shown}: {}x{}, {pixels} differing pixels, max channel delta {max}{bounds}",
            pa.width, pa.height
        );
        if pixels > 0 {
            differing += 1;
        }
    }
    println!("{} pictures, {differing} differing", names.len());
    if compared == 0 {
        return Err("no picture was in both folders to compare".to_string());
    }
    Ok(differing > 0)
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).peekable();
    let (mut corner, mut skip, mut folders) = ((0, 0), Vec::new(), Vec::new());
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--ignore-corner" => {
                let size = args.next().unwrap_or_default();
                let parsed = size
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
                let Some(parsed) = parsed else {
                    eprintln!("shots-diff: --ignore-corner needs a size like 160x40");
                    return ExitCode::from(2);
                };
                corner = parsed;
            }
            "--skip" => {
                let Some(path) = args.next() else {
                    eprintln!("shots-diff: --skip needs a picture's path");
                    return ExitCode::from(2);
                };
                skip.push(path);
            }
            _ => folders.push(arg),
        }
    }
    let [a, b] = folders.as_slice() else {
        eprintln!("usage: shots-diff [--ignore-corner WxH] [--skip <path>]... <folder> <folder>");
        return ExitCode::from(2);
    };
    match run(Path::new(a), Path::new(b), corner, &skip) {
        Ok(false) => ExitCode::SUCCESS,
        Ok(true) => ExitCode::from(1),
        Err(e) => {
            eprintln!("shots-diff: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty folder of this test's own below the target folder.
    fn folder(test: &str, name: &str) -> PathBuf {
        let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
            PathBuf::from,
        );
        let dir = target.join("shots-diff-tests").join(test).join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes a `w` x `h` picture of one color, with the top right pixel `corner` instead.
    fn picture(dir: &Path, name: &str, (w, h): (u32, u32), color: u8, corner: u8) {
        let mut rgba = vec![color; (w * h * 4) as usize];
        let at = (w as usize - 1) * 4;
        rgba[at..at + 3].fill(corner);
        let file = std::fs::File::create(dir.join(name)).unwrap();
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&rgba)
            .unwrap();
    }

    #[test]
    fn empty_folders_are_an_error() {
        let (a, b) = (folder("empty", "a"), folder("empty", "b"));
        assert!(run(&a, &b, (0, 0), &[]).is_err());
    }

    #[test]
    fn a_picture_on_one_side_differs() {
        let (a, b) = (folder("one-side", "a"), folder("one-side", "b"));
        picture(&a, "x.png", (4, 4), 9, 9);
        picture(&b, "x.png", (4, 4), 9, 9);
        picture(&a, "y.png", (4, 4), 9, 9);
        assert_eq!(run(&a, &b, (0, 0), &[]), Ok(true));
    }

    #[test]
    fn only_skipped_pictures_are_an_error() {
        let (a, b) = (folder("skipped", "a"), folder("skipped", "b"));
        picture(&a, "x.png", (4, 4), 9, 9);
        picture(&b, "x.png", (4, 4), 9, 9);
        assert!(run(&a, &b, (0, 0), &["x.png".to_string()]).is_err());
    }

    #[test]
    fn a_size_mismatch_differs() {
        let (a, b) = (folder("size", "a"), folder("size", "b"));
        picture(&a, "x.png", (4, 4), 9, 9);
        picture(&b, "x.png", (5, 4), 9, 9);
        assert_eq!(run(&a, &b, (0, 0), &[]), Ok(true));
    }

    #[test]
    fn a_skip_that_matches_nothing_is_an_error() {
        let (a, b) = (folder("typo", "a"), folder("typo", "b"));
        picture(&a, "x.png", (4, 4), 9, 9);
        picture(&b, "x.png", (4, 4), 9, 9);
        let error = run(&a, &b, (0, 0), &["y.png".to_string()]).unwrap_err();
        assert!(error.contains("y.png"), "{error}");
    }

    #[test]
    fn a_difference_in_the_masked_corner_passes() {
        let (a, b) = (folder("corner", "a"), folder("corner", "b"));
        picture(&a, "x.png", (8, 8), 9, 9);
        picture(&b, "x.png", (8, 8), 9, 200);
        assert_eq!(run(&a, &b, (2, 2), &[]), Ok(false));
        assert_eq!(run(&a, &b, (0, 0), &[]), Ok(true));
    }

    #[test]
    fn a_real_difference_fails() {
        let (a, b) = (folder("real", "a"), folder("real", "b"));
        picture(&a, "x.png", (8, 8), 9, 9);
        picture(&b, "x.png", (8, 8), 10, 10);
        assert_eq!(run(&a, &b, (2, 2), &[]), Ok(true));
    }

    #[test]
    fn identical_pictures_pass() {
        let (a, b) = (folder("same", "a"), folder("same", "b"));
        picture(&a, "x.png", (8, 8), 9, 9);
        picture(&b, "x.png", (8, 8), 9, 9);
        assert_eq!(run(&a, &b, (0, 0), &[]), Ok(false));
    }
}
