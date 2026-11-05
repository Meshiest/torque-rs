//! The mesh input's verification at its full counts on the 20 generated glTF and OBJ fixtures (closed solids, open meshes,
//! authored hulls, rocks with volumes, a mix, and a terrain of 200,344 triangles cut into pieces): 100,000 surface points,
//! 1,000,000 segments and 1,000,000 points a fixture.
//! `cargo run --release -p torque-interior-builder --features verify --example dif-mesh-verify [share [seeds]]`; prints one
//! line a fixture and exits with an error when a check fails.

use torque_interior_builder::mesh::MeshOptions;
use torque_interior_builder::verify::Counts;
use torque_interior_builder::verify::fixtures::{fixture, read};
use torque_interior_builder::verify::mesh::run_mesh;

fn main() -> std::process::ExitCode {
    let share: f64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(1.0);
    let counts = Counts::scaled(share);
    let mut seeds: Vec<u64> = std::env::args()
        .skip(2)
        .filter_map(|a| a.parse().ok())
        .collect();
    if seeds.is_empty() {
        seeds = (0..20).collect();
    }
    let mut failed = 0;
    for &seed in &seeds {
        let f = fixture(seed);
        let started = std::time::Instant::now();
        let result = read(&f).and_then(|input| {
            if input.objects.len() != f.volumes {
                return Err(format!(
                    "{} mission objects, want {}",
                    input.objects.len(),
                    f.volumes
                ));
            }
            run_mesh(&input, &MeshOptions::default(), counts, seed)
        });
        match result {
            Ok(r) => {
                println!(
                    "fixture {seed:2} ({}, {} triangles, {:.1} s): {}",
                    f.name,
                    f.triangles,
                    started.elapsed().as_secs_f64(),
                    r.line()
                );
                failed += usize::from(!r.clean());
                if let Some(first) = &r.first {
                    println!("  first: {first}");
                }
            }
            Err(e) => {
                println!("fixture {seed:2} ({}): {e}", f.name);
                failed += 1;
            }
        }
    }
    println!("{failed} of {} fixtures failed", seeds.len());
    if failed == 0 {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
