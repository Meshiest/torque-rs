//! Renders the pictures of the office's prop catalog: each kind's shape on a plain floor, seen by a fixed camera, 128 x 128 pixels,
//! offscreen (no window opens).
//!
//! `cargo run -p torque-example-level-demo --release --example catalog-icons [-- <folder>]` writes `<kind>.png` for each kind of
//! prop the office has; the folder defaults to the catalog mod's `catalog/icons`.

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), String> {
    use std::path::{Path, PathBuf};

    let dir = std::env::args().nth(1).map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog/icons"),
        PathBuf::from,
    );
    for path in torque_example_level_demo::icons::write_icons(&dir, 128)? {
        println!("{}", path.file_name().unwrap_or_default().to_string_lossy());
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn main() {}
