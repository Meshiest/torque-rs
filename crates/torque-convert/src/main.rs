//! `torque-convert`: models and maps into the files the engine reads.
//!
//! ```text
//! torque-convert <in.glb|in.gltf|in.obj|in.bbmodel|in.dts> <out.dts|out.glb> [--up y|z] [--scale <meters a unit, 1>]
//!     [--rate <keys a second, 30>] [--no-hull] [--force]
//! torque-convert <in.map> <out.dif> ... (see the interior module)
//! torque-convert <in.glb|in.gltf|in.obj|in.bbmodel> <out.dif> ... (see the mesh module)
//! ```
//!
//! A model is read into the model layer (`torque-import`) and written as a `.dts` (images the model carries are written beside
//! it, named as its materials name them; one that would replace a file already there, or two of one name, is an error unless
//! `--force`) or a `.glb`. A `.map` is built into an interior; a model with a `.dif` output too.
//!
//! # Maintaining
//!
//! **The user's files beside the output are theirs.** Images are written with `create_new` after a check of every name, so a
//! name that exists, or that two images share (case folded), is an error and nothing is written, unless `--force`
//! (`model::tests::an_image_never_replaces_a_file_already_beside_the_output_unless_forced`,
//! `two_images_of_one_name_are_refused_and_so_is_a_superscript_device_name`). File names follow the script host's rules (device names,
//! superscript digits) plus a leading-dot rule; `the_name_rules_agree_with_the_script_hosts` holds the two together.

mod interior;
mod mesh;
mod model;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let map = args
        .iter()
        .any(|a| a.to_ascii_lowercase().ends_with(".map"));
    let dif = args
        .iter()
        .any(|a| a.to_ascii_lowercase().ends_with(".dif"));
    match args.first() {
        Some(_) if map => interior::run(&args),
        Some(_) if dif => mesh::run(&args),
        Some(_) => model::run(&args),
        None => {
            eprintln!(
                "usage: torque-convert <in.glb|in.gltf|in.obj|in.bbmodel|in.dts> <out.dts|out.glb> [options], or \
                 <in.map|in.glb|in.gltf|in.obj|in.bbmodel> <out.dif> [options]"
            );
            ExitCode::FAILURE
        }
    }
}
