//! What the example games share.
//!
//! - [`KitAssets`]: the asset kit (`torque-example-assets`) made in memory and read as files under [`KIT_DIR`], the way the
//!   renderer's shape and texture loaders read any game's files.
//! - [`view::WorldView`]: the per-frame world view. A frame is an [`torque_render_scene::hd::HdScene`] (daylight, a floor, the
//!   frame's moving shapes), drawn by the enhanced renderer into the rectangle the canvas's `GameTSCtrl` gives the world, then the
//!   canvas's GUI drawn over it by the GUI pass.
//! - [`view::SceneShape`]: a shape loaded from the kit, posed by a `ShapeInstance` and turned into a moving object of the frame.
//! - [`orbit::Orbit`]: a third-person camera that circles a point.
//! - [`window::WindowSurface`]: a window's GPU device and surface, kept to its size, on the examples' [`window::BACKENDS`] (Vulkan
//!   first: its pipelines are made in well under a second); [`window::offscreen_gpu`] the same for pictures and tests.
//! - [`draw_cache`]: the GUI's draw cache with the fonts the examples use (the system's TrueType faces where it has them).

pub mod orbit;
pub mod view;
pub mod window;

use std::collections::HashMap;
use std::io;

use torque::fonts::{Fallback, FontCache, NoFallback};
use torque::gui::DrawCache;
use torque_render_scene::Assets;

/// The folder the kit's files are read from.
pub const KIT_DIR: &str = "kit";

/// The asset kit as files: `kit/cube.dts`, `kit/figure.dts`, `kit/grid.png` and the rest, made once in memory.
pub struct KitAssets {
    files: HashMap<String, Vec<u8>>,
}

impl KitAssets {
    /// Makes every asset of the kit.
    pub fn new() -> io::Result<KitAssets> {
        let files = torque_example_assets::all()?
            .into_iter()
            .map(|a| (kit_path(a.name), a.bytes))
            .collect();
        Ok(KitAssets { files })
    }

    /// The files' paths.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }
}

impl Assets for KitAssets {
    /// A file by its path, ignoring case and with `\` read as `/`.
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.files
            .get(&path.replace('\\', "/").to_ascii_lowercase())
            .cloned()
    }
}

/// The path of the kit's file `name` (`figure.dts` is `kit/figure.dts`).
pub fn kit_path(name: &str) -> String {
    format!("{KIT_DIR}/{}", name.to_ascii_lowercase())
}

/// The GUI's draw cache: fonts from `.uft` files in `fonts/` when there are any, else rasterised from the system's TrueType
/// face of the profile's name (Windows' fonts folder); with neither, text is not drawn.
pub fn draw_cache() -> DrawCache {
    let fallback: Box<dyn Fallback> = match torque::fonts::system::SystemFonts::new() {
        Some(system) => Box::new(system),
        None => Box::new(NoFallback),
    };
    DrawCache::new(Box::new(FontCache::new("fonts", fallback)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kit_reads_by_path_in_any_case() {
        let kit = KitAssets::new().unwrap();
        assert!(kit.read("kit/figure.dts").is_some());
        assert_eq!(kit.read("KIT\\Grid.PNG"), kit.read("kit/grid.png"));
        assert!(kit.read("kit/missing.png").is_none());
        assert_eq!(
            kit.paths().count(),
            torque_example_assets::all().unwrap().len()
        );
    }
}
