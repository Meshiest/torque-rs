//! Interiors: Torque's `.dif` files read as data, and (`map-builder` feature) Quake or Valve 220 `.map` files built into one.
//!
//! - [`DifFormatPlugin`]: `.dif` as [`Dif`], read as the server's `InteriorInstance` reads it today ([`dif::parse`]): files up
//!   to [`MAX_DIF_BYTES`], from any source (the reader is hardened and fuzzed: `torque_maps::fuzz`).
//! - `QuakeMapFormatPlugin` (`map-builder` feature): `.map` as `BuiltInterior`, read and built as `torque-convert` does
//!   (`torque_interior_builder::map::parse` with the plugin's scale and no texture sizes, then `torque_interior_builder::build`
//!   with the default options): files up to `torque_interior_builder::map::MAX_MAP_BYTES`, from trusted sources only (the
//!   reader caps what it reads, but it is not fuzzed, and a build is a long piece of work).

use torque_engine::assets::{LoadCx, Spec};
use torque_engine::{EngineBuilder, Plugin, PluginId};
use torque_maps::MAX_DIF_BYTES;
use torque_maps::dif::{self, Dif};

/// The interior of a `.dif`'s bytes, as the server reads it.
pub fn read_dif(bytes: &[u8]) -> Result<Dif, String> {
    dif::parse(bytes).map_err(|e| e.to_string())
}

/// Torque interiors: `.dif` as [`Dif`], from any source (module docs).
#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "torque.format-dif")]
pub struct DifFormatPlugin;

impl DifFormatPlugin {
    /// What the loader takes.
    pub const SPEC: Spec = Spec::untrusted(MAX_DIF_BYTES as u64);
}

impl Plugin for DifFormatPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .assets()
            .loader(&["dif"], Self::SPEC, |bytes, _: &LoadCx<'_>| {
                read_dif(bytes)
            });
    }
}

#[cfg(feature = "map-builder")]
pub use quake::*;

#[cfg(feature = "map-builder")]
mod quake {
    use torque_engine::assets::{LoadCx, Spec};
    use torque_engine::{EngineBuilder, Plugin, PluginId};
    use torque_interior_builder::map::{self, MapOptions};
    use torque_interior_builder::mission::MissionObject;
    use torque_interior_builder::{Built, Options, build};

    /// An interior built from a `.map`: what was built, its `.dif` bytes, the mission objects that come with it (placed with
    /// the interior's instance, [`torque_interior_builder::mission::fragment`]) and the reader's warnings.
    #[derive(Clone, Debug, PartialEq)]
    pub struct BuiltInterior {
        /// What was built (its [`Built::dif`] is the interior).
        pub built: Built,
        /// The `.dif`'s bytes.
        pub bytes: Vec<u8>,
        /// The water volumes, triggers, zones and point entities the file has.
        pub objects: Vec<MissionObject>,
        /// What the reader left out or guessed.
        pub warnings: Vec<String>,
    }

    /// The interior of a `.map`'s bytes, `units_per_meter` map units a unit (module docs).
    pub fn read_map(bytes: &[u8], units_per_meter: f64) -> Result<BuiltInterior, String> {
        let text = String::from_utf8_lossy(bytes);
        let parsed = map::parse(
            &text,
            &MapOptions {
                units_per_meter,
                ..MapOptions::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let built = build(&parsed.brushes, &Options::default()).map_err(|e| e.to_string())?;
        let bytes = built.bytes().map_err(|e| e.to_string())?;
        Ok(BuiltInterior {
            built,
            bytes,
            objects: parsed.objects,
            warnings: parsed.warnings,
        })
    }

    /// Quake and Valve 220 maps: `.map` as [`BuiltInterior`], from trusted sources only (module docs). `units_per_meter`:
    /// map units a unit of the interior (32 by default, as `torque-convert`'s).
    #[derive(Clone, Copy, Debug, PluginId)]
    #[plugin(id = "torque.format-quake-map")]
    pub struct QuakeMapFormatPlugin {
        /// Map units a unit of the interior.
        pub units_per_meter: f64,
    }

    impl Default for QuakeMapFormatPlugin {
        fn default() -> QuakeMapFormatPlugin {
            QuakeMapFormatPlugin {
                units_per_meter: 32.0,
            }
        }
    }

    impl QuakeMapFormatPlugin {
        /// What the loader takes.
        pub const SPEC: Spec = Spec::trusted(map::MAX_MAP_BYTES as u64);
    }

    impl Plugin for QuakeMapFormatPlugin {
        fn build(&self, engine: &mut EngineBuilder) {
            let scale = self.units_per_meter;
            engine
                .assets()
                .loader(&["map"], Self::SPEC, move |bytes, _: &LoadCx<'_>| {
                    read_map(bytes, scale)
                });
        }
    }
}
