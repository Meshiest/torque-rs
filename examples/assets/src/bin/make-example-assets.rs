//! Writes the example asset kit into a folder: `make-example-assets [dir]` (default `example-assets`).

use std::path::PathBuf;

fn main() -> std::io::Result<()> {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "example-assets".to_string()),
    );
    for path in torque_example_assets::write_all(&dir)? {
        println!("{}", path.display());
    }
    Ok(())
}
