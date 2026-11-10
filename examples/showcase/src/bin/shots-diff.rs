//! Compares two folders of pictures: `shots-diff [options] <a> <b>`.
//!
//! Every PNG under either folder is matched by its path below the folder. For each one the line says whether the sizes match,
//! how many pixels differ (any channel) and the largest difference in any channel. A picture only one folder has counts as a
//! difference.
//!
//! Options:
//! - `--ignore-corner WxH` leaves the top right corner of that size out of every comparison (the corner readout of the frame
//!   rate, which is measured on the clock and so differs from run to run).
//! - `--max-delta N` and `--max-pixels M` (both default 0) set a tolerance: a picture whose differing pixels all differ by at
//!   most `N` in every channel, and number at most `M`, passes as "near". Near pictures are listed apart from exact matches at
//!   the end of the output, so they stay visible; a picture beyond either limit differs as usual.
//! - `--skip <path>` leaves out the picture at that path below the folders (named in the output, never silently). A skip that
//!   matches no picture is an error, since it is most likely a typo.
//! - `--subset` is for diffing part of the full set (one example's folder): a skip that matches nothing is then reported
//!   instead of failing.
//!
//! The exit code is 1 when any picture differs, 2 for a usage or read error, else 0.

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

/// What to leave out of a comparison and how much difference to accept.
#[derive(Default)]
struct Options {
    /// Size of the top right corner left out of every comparison.
    corner: (u32, u32),
    /// Pictures left out, by path below the folders.
    skip: Vec<String>,
    /// A skip that matches no picture is reported, not an error.
    subset: bool,
    /// The largest channel difference a near match may have.
    max_delta: u8,
    /// How many pixels a near match may differ in.
    max_pixels: usize,
}

/// How a comparison came out.
#[derive(Debug, Default, PartialEq, Eq)]
struct Report {
    /// Pictures identical in both folders.
    exact: usize,
    /// Pictures that differ, but within the tolerance.
    near: Vec<String>,
    /// Pictures that differ beyond the tolerance, are missing from a folder, or have different sizes.
    differing: usize,
}

impl Report {
    fn failed(&self) -> bool {
        self.differing > 0
    }
}

fn run(a: &Path, b: &Path, options: &Options) -> Result<Report, String> {
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
    for unmatched in options.skip.iter().filter(|s| !shown_names.contains(s)) {
        if !options.subset {
            return Err(format!("--skip {unmatched} matches no picture"));
        }
        println!("{unmatched}: skipped, not in this set");
    }
    let mut report = Report::default();
    let mut compared = 0usize;
    for name in &names {
        let shown = name.to_string_lossy().replace('\\', "/");
        if options.skip.contains(&shown) {
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
            report.differing += 1;
            continue;
        }
        let (pa, pb) = (read(&a.join(name))?, read(&b.join(name))?);
        compared += 1;
        if (pa.width, pa.height) != (pb.width, pb.height) {
            println!(
                "{shown}: size {}x{} against {}x{}",
                pa.width, pa.height, pb.width, pb.height
            );
            report.differing += 1;
            continue;
        }
        let corner = options.corner;
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
        let verdict = if pixels == 0 {
            report.exact += 1;
            ""
        } else if max <= options.max_delta && pixels <= options.max_pixels {
            report.near.push(shown.clone());
            ", near (within the tolerance)"
        } else {
            report.differing += 1;
            ""
        };
        println!(
            "{shown}: {}x{}, {pixels} differing pixels, max channel delta {max}{bounds}{verdict}",
            pa.width, pa.height
        );
    }
    println!(
        "{} pictures, {} exact, {} near, {} differing",
        names.len(),
        report.exact,
        report.near.len(),
        report.differing
    );
    if !report.near.is_empty() {
        println!(
            "near (at most {} per channel, at most {} pixels):",
            options.max_delta, options.max_pixels
        );
        for shown in &report.near {
            println!("  {shown}");
        }
    }
    if compared == 0 {
        return Err("no picture was in both folders to compare".to_string());
    }
    Ok(report)
}

/// The value after a numeric switch, or the message to print.
fn number<T: std::str::FromStr>(
    args: &mut impl Iterator<Item = String>,
    switch: &str,
) -> Result<T, String> {
    args.next()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| format!("{switch} needs a whole number"))
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<(Options, Vec<String>), String> {
    let mut args = args.into_iter();
    let (mut options, mut folders) = (Options::default(), Vec::new());
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--ignore-corner" => {
                let size = args.next().unwrap_or_default();
                let parsed = size
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
                options.corner = parsed.ok_or("--ignore-corner needs a size like 160x40")?;
            }
            "--skip" => {
                let path = args.next().ok_or("--skip needs a picture's path")?;
                options.skip.push(path);
            }
            "--subset" => options.subset = true,
            "--max-delta" => options.max_delta = number(&mut args, "--max-delta")?,
            "--max-pixels" => options.max_pixels = number(&mut args, "--max-pixels")?,
            _ => folders.push(arg),
        }
    }
    Ok((options, folders))
}

fn main() -> ExitCode {
    let (options, folders) = match parse(std::env::args().skip(1)) {
        Ok(parsed) => parsed,
        Err(e) => {
            eprintln!("shots-diff: {e}");
            return ExitCode::from(2);
        }
    };
    let [a, b] = folders.as_slice() else {
        eprintln!(
            "usage: shots-diff [--ignore-corner WxH] [--max-delta N] [--max-pixels M] [--skip <path>]... [--subset] <folder> <folder>"
        );
        return ExitCode::from(2);
    };
    match run(Path::new(a), Path::new(b), &options) {
        Ok(report) if report.failed() => ExitCode::from(1),
        Ok(_) => ExitCode::SUCCESS,
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

    /// Writes a `w` x `h` picture of one color whose first `count` pixels are `delta` brighter.
    fn noisy(dir: &Path, name: &str, (w, h): (u32, u32), color: u8, count: usize, delta: u8) {
        let mut rgba = vec![color; (w * h * 4) as usize];
        for pixel in rgba.as_chunks_mut::<4>().0.iter_mut().take(count) {
            pixel[..3].fill(color + delta);
        }
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

    fn skipping(paths: &[&str]) -> Options {
        Options {
            skip: paths.iter().map(ToString::to_string).collect(),
            ..Options::default()
        }
    }

    fn tolerant(max_delta: u8, max_pixels: usize) -> Options {
        Options {
            max_delta,
            max_pixels,
            ..Options::default()
        }
    }

    #[test]
    fn empty_folders_are_an_error() {
        let (a, b) = (folder("empty", "a"), folder("empty", "b"));
        assert!(run(&a, &b, &Options::default()).is_err());
    }

    #[test]
    fn a_picture_on_one_side_differs() {
        let (a, b) = (folder("one-side", "a"), folder("one-side", "b"));
        picture(&a, "x.png", (4, 4), 9, 9);
        picture(&b, "x.png", (4, 4), 9, 9);
        picture(&a, "y.png", (4, 4), 9, 9);
        assert!(run(&a, &b, &Options::default()).unwrap().failed());
    }

    #[test]
    fn only_skipped_pictures_are_an_error() {
        let (a, b) = (folder("skipped", "a"), folder("skipped", "b"));
        picture(&a, "x.png", (4, 4), 9, 9);
        picture(&b, "x.png", (4, 4), 9, 9);
        assert!(run(&a, &b, &skipping(&["x.png"])).is_err());
    }

    #[test]
    fn a_size_mismatch_differs() {
        let (a, b) = (folder("size", "a"), folder("size", "b"));
        picture(&a, "x.png", (4, 4), 9, 9);
        picture(&b, "x.png", (5, 4), 9, 9);
        assert!(run(&a, &b, &Options::default()).unwrap().failed());
    }

    #[test]
    fn a_skip_that_matches_nothing_is_an_error() {
        let (a, b) = (folder("typo", "a"), folder("typo", "b"));
        picture(&a, "x.png", (4, 4), 9, 9);
        picture(&b, "x.png", (4, 4), 9, 9);
        let error = run(&a, &b, &skipping(&["y.png"])).unwrap_err();
        assert!(error.contains("y.png"), "{error}");
    }

    #[test]
    fn a_typo_still_fails_next_to_a_real_skip() {
        let (a, b) = (folder("typo-real", "a"), folder("typo-real", "b"));
        for dir in [&a, &b] {
            picture(dir, "x.png", (4, 4), 9, 9);
            picture(dir, "y.png", (4, 4), 9, 9);
        }
        assert!(run(&a, &b, &skipping(&["x.png"])).is_ok());
        let error = run(&a, &b, &skipping(&["x.png", "z.png"])).unwrap_err();
        assert!(error.contains("z.png"), "{error}");
    }

    #[test]
    fn a_subset_run_tolerates_a_skip_from_another_folder() {
        let (a, b) = (folder("subset", "a"), folder("subset", "b"));
        picture(&a, "x.png", (4, 4), 9, 9);
        picture(&b, "x.png", (4, 4), 9, 9);
        let mut options = skipping(&["other-example/hill.png"]);
        assert!(run(&a, &b, &options).is_err());
        options.subset = true;
        let report = run(&a, &b, &options).unwrap();
        assert!(!report.failed());
        assert_eq!(report.exact, 1);
    }

    #[test]
    fn a_subset_run_still_skips_what_is_there() {
        let (a, b) = (folder("subset-hit", "a"), folder("subset-hit", "b"));
        picture(&a, "x.png", (4, 4), 9, 9);
        picture(&b, "x.png", (4, 4), 9, 9);
        picture(&a, "y.png", (4, 4), 9, 9);
        picture(&b, "y.png", (4, 4), 90, 90);
        let mut options = skipping(&["y.png", "gone.png"]);
        options.subset = true;
        let report = run(&a, &b, &options).unwrap();
        assert!(!report.failed());
        assert_eq!(report.exact, 1);
    }

    #[test]
    fn a_difference_in_the_masked_corner_passes() {
        let (a, b) = (folder("corner", "a"), folder("corner", "b"));
        picture(&a, "x.png", (8, 8), 9, 9);
        picture(&b, "x.png", (8, 8), 9, 200);
        let masked = Options {
            corner: (2, 2),
            ..Options::default()
        };
        assert!(!run(&a, &b, &masked).unwrap().failed());
        assert!(run(&a, &b, &Options::default()).unwrap().failed());
    }

    #[test]
    fn a_real_difference_fails() {
        let (a, b) = (folder("real", "a"), folder("real", "b"));
        picture(&a, "x.png", (8, 8), 9, 9);
        picture(&b, "x.png", (8, 8), 10, 10);
        let masked = Options {
            corner: (2, 2),
            ..Options::default()
        };
        assert!(run(&a, &b, &masked).unwrap().failed());
    }

    #[test]
    fn identical_pictures_pass_as_exact() {
        let (a, b) = (folder("same", "a"), folder("same", "b"));
        picture(&a, "x.png", (8, 8), 9, 9);
        picture(&b, "x.png", (8, 8), 9, 9);
        let report = run(&a, &b, &tolerant(1, 32)).unwrap();
        assert_eq!(
            report,
            Report {
                exact: 1,
                near: Vec::new(),
                differing: 0
            }
        );
    }

    #[test]
    fn a_difference_within_the_tolerance_is_near() {
        let (a, b) = (folder("near", "a"), folder("near", "b"));
        for dir in [&a, &b] {
            picture(dir, "same.png", (8, 8), 9, 9);
        }
        noisy(&a, "x.png", (8, 8), 9, 0, 0);
        noisy(&b, "x.png", (8, 8), 9, 5, 1);
        let report = run(&a, &b, &tolerant(1, 5)).unwrap();
        assert!(!report.failed());
        assert_eq!(report.exact, 1);
        assert_eq!(report.near, ["x.png"]);
    }

    #[test]
    fn the_tolerance_is_off_by_default() {
        let (a, b) = (folder("strict", "a"), folder("strict", "b"));
        noisy(&a, "x.png", (8, 8), 9, 0, 0);
        noisy(&b, "x.png", (8, 8), 9, 1, 1);
        let report = run(&a, &b, &Options::default()).unwrap();
        assert!(report.failed());
        assert!(report.near.is_empty());
    }

    #[test]
    fn a_difference_over_the_delta_fails() {
        let (a, b) = (folder("delta", "a"), folder("delta", "b"));
        noisy(&a, "x.png", (8, 8), 9, 0, 0);
        noisy(&b, "x.png", (8, 8), 9, 2, 2);
        let report = run(&a, &b, &tolerant(1, 32)).unwrap();
        assert!(report.failed());
        assert!(report.near.is_empty());
    }

    #[test]
    fn a_difference_over_the_pixel_count_fails() {
        let (a, b) = (folder("pixels", "a"), folder("pixels", "b"));
        noisy(&a, "x.png", (8, 8), 9, 0, 0);
        noisy(&b, "x.png", (8, 8), 9, 33, 1);
        assert!(run(&a, &b, &tolerant(1, 32)).unwrap().failed());
        noisy(&b, "x.png", (8, 8), 9, 32, 1);
        assert_eq!(run(&a, &b, &tolerant(1, 32)).unwrap().near, ["x.png"]);
    }

    #[test]
    fn one_picture_over_the_limit_fails_the_run_beside_near_ones() {
        let (a, b) = (folder("mixed", "a"), folder("mixed", "b"));
        noisy(&a, "near.png", (8, 8), 9, 0, 0);
        noisy(&b, "near.png", (8, 8), 9, 3, 1);
        noisy(&a, "far.png", (8, 8), 9, 0, 0);
        noisy(&b, "far.png", (8, 8), 9, 3, 40);
        let report = run(&a, &b, &tolerant(1, 32)).unwrap();
        assert_eq!(report.near, ["near.png"]);
        assert_eq!(report.differing, 1);
    }

    #[test]
    fn switches_parse() {
        let args = [
            "--max-delta",
            "1",
            "--max-pixels",
            "32",
            "--subset",
            "--skip",
            "x.png",
            "--ignore-corner",
            "160x40",
            "a",
            "b",
        ]
        .map(String::from);
        let (options, folders) = parse(args).unwrap();
        assert_eq!(
            (options.max_delta, options.max_pixels, options.subset),
            (1, 32, true)
        );
        assert_eq!(
            (options.corner, options.skip),
            ((160, 40), vec!["x.png".to_string()])
        );
        assert_eq!(folders, ["a", "b"]);
        assert!(parse(["--max-delta", "many"].map(String::from)).is_err());
        assert!(parse(["--max-pixels"].map(String::from)).is_err());
    }
}
