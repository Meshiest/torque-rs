//! `torque-convert`: models and maps into the files the engine reads.
//!
//! ```text
//! torque-convert <in.glb|in.gltf|in.obj|in.bbmodel|in.dts> <out.dts|out.glb> [--up y|z] [--scale <meters a unit, 1>]
//!     [--rate <keys a second, 30>] [--no-hull]
//! torque-convert <in.map> <out.dif> ... (see the interior module)
//! torque-convert <in.glb|in.gltf|in.obj|in.bbmodel> <out.dif> ... (see the mesh module)
//! ```
//!
//! A model is read into the model layer (`torque-import`) and written as a `.dts` (images the model carries are written beside
//! it, named as its materials name them) or a `.glb`. A `.map` is built into an interior; a model with a `.dif` output too.

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
