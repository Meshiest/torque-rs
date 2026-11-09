//! A mesh (`.glb`, `.gltf`, `.obj`, `.bbmodel`) into one or more `.dif`s and a `.mis` fragment.
//!
//! ```text
//! torque-convert <in.glb|in.gltf|in.obj|in.bbmodel> <out.dif> [--hulls auto|authored|slabs] [--slab <thickness, 0.1>]
//!     [--lightmap-texel <units, 0.5>] [--up y|z] [--scale <meters a unit, 1>] [--mission <out.mis>] [--name <object name>]
//!     [--file <interiorFile path>] [--position "x y z"] [--rotation "ax ay az degrees"] [--instance-scale "x y z"]
//! ```
//!
//! A set too large for one interior is cut into pieces written as `out_1.dif`, `out_2.dif`, ... (one `InteriorInstance` each,
//! all at the same place). Images the model carries are written beside the output under their material names. The fragment
//! holds the instances and the model's mission volumes (`water*`, `trigger*`, `physical_zone*`), placed with them.

use std::path::Path;
use std::process::ExitCode;

use torque_import::Options as ReadOptions;
use torque_interior_builder::mesh::{HullMode, MeshInput, MeshOptions, build_mesh};
use torque_interior_builder::mission::{Placement, fragment};

use crate::model;

fn usage() -> ExitCode {
    eprintln!(
        "usage: torque-convert <in.glb|in.gltf|in.obj|in.bbmodel> <out.dif> [--hulls auto|authored|slabs] [--slab T] \
         [--lightmap-texel T] [--up y|z] [--scale N] [--mission OUT.mis] [--name NAME] [--file PATH] [--position \"x y z\"] \
         [--rotation \"ax ay az deg\"] [--instance-scale \"x y z\"] [--force]"
    );
    ExitCode::FAILURE
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

/// The command for a mesh input with a `.dif` output (module docs).
pub fn run(args: &[String]) -> ExitCode {
    let mut plain = Vec::new();
    let mut read_options = ReadOptions {
        box_hull: false,
        ..ReadOptions::default()
    };
    let mut options = MeshOptions::default();
    let mut mission: Option<String> = None;
    let mut name = String::new();
    let mut file: Option<String> = None;
    let mut at = Placement::default();
    let mut force = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned();
        let ok = match a.as_str() {
            "--hulls" => value()
                .and_then(|v| match v.as_str() {
                    "auto" => Some(HullMode::Auto),
                    "authored" => Some(HullMode::Authored),
                    "slabs" => Some(HullMode::Slabs),
                    _ => None,
                })
                .map(|m| options.hulls = m)
                .is_some(),
            "--slab" => value()
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v > 0.0)
                .map(|v| options.slab = v)
                .is_some(),
            "--lightmap-texel" => value()
                .and_then(|v| v.parse().ok())
                .map(|v| options.build.lightmap_texel = v)
                .is_some(),
            "--up" => value()
                .filter(|v| v == "y" || v == "z")
                .map(|v| read_options.up = if v == "z" { 'z' } else { 'y' })
                .is_some(),
            "--scale" => value()
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|v| v.is_finite() && *v > 0.0)
                .map(|v| read_options.scale = v)
                .is_some(),
            "--mission" => value().map(|v| mission = Some(v)).is_some(),
            "--name" => value().map(|v| name = v).is_some(),
            "--force" => {
                force = true;
                true
            }
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
    let m = match model::read(input, &read_options) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{input}: {e}");
            return ExitCode::FAILURE;
        }
    };
    for n in &m.notes {
        eprintln!("note: {n}");
    }
    let mesh = match MeshInput::from_model(&m) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{input}: {e}");
            return ExitCode::FAILURE;
        }
    };
    for n in &mesh.notes {
        eprintln!("note: {n}");
    }
    let built = match build_mesh(&mesh, &options) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{input}: {e}");
            return ExitCode::FAILURE;
        }
    };
    for n in &built.plan.notes {
        eprintln!("note: {n}");
    }
    let out = Path::new(output);
    let count = built.pieces.len();
    let stem = out.file_stem().map_or_else(
        || "interior".to_string(),
        |s| s.to_string_lossy().into_owned(),
    );
    let mut text = String::new();
    let (mut surfaces, mut hulls, mut bytes_total) = (0, 0, 0);
    for (k, piece) in built.pieces.iter().enumerate() {
        let path = if count == 1 {
            out.to_path_buf()
        } else {
            out.with_file_name(format!("{stem}_{}.dif", k + 1))
        };
        let bytes = match piece.built.bytes() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        };
        if let Err(e) = std::fs::write(&path, &bytes) {
            eprintln!("{}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        bytes_total += bytes.len();
        let i = &piece.built.dif.details[0];
        surfaces += i.surfaces.len();
        hulls += i.hulls.hulls.len();
        let file_name = path
            .file_name()
            .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
        let file = match (&file, count) {
            (Some(f), 1) => f.clone(),
            (Some(f), _) => match f.rsplit_once('/') {
                Some((dir, _)) => format!("{dir}/{file_name}"),
                None => file_name,
            },
            (None, _) => file_name,
        };
        let piece_name = if count == 1 || name.is_empty() {
            name.clone()
        } else {
            format!("{name}_{}", k + 1)
        };
        let objects = if k == 0 { &mesh.objects[..] } else { &[] };
        text.push_str(&fragment(&piece_name, &file, &at, objects));
    }
    let mis = mission.map_or_else(|| out.with_extension("mis"), Into::into);
    if let Err(e) = std::fs::write(&mis, text) {
        eprintln!("{}: {e}", mis.display());
        return ExitCode::FAILURE;
    }
    let images = match model::write_images(&m, out, force) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("{output}: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "{output}: {count} interiors, {bytes_total} bytes; {} polygons, {surfaces} surfaces, {hulls} hulls; {} mission objects \
         in {}; {images} images written beside it",
        built.plan.polygons.len(),
        mesh.objects.len(),
        mis.display()
    );
    ExitCode::SUCCESS
}
