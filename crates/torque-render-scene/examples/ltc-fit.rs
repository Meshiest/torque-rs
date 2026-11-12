//! Fits the area lights' table of linearly transformed cosines again ([`torque_render_scene::hd::ltc::fit_table`]) and writes it
//! over `src/hd/ltc_ggx.bin`, or to the path given. Four threads (one roughness row a task).
//!
//! `cargo run -p torque-render-scene --release --example ltc-fit -- [path]`

use std::path::PathBuf;

use torque_render_scene::hd::ltc;

fn main() -> Result<(), String> {
    let path = std::env::args().nth(1).map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/hd/ltc_ggx.bin"),
        PathBuf::from,
    );
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    let cells = pool.install(ltc::fit_table);
    std::fs::write(&path, ltc::to_bytes(&cells)).map_err(|e| e.to_string())?;
    println!(
        "{} cells in {:.1} s -> {}",
        cells.len(),
        started.elapsed().as_secs_f64(),
        path.display()
    );
    Ok(())
}
