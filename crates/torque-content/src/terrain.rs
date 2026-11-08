//! Terrains: Torque's legacy `.ter` files read as data.
//!
//! [`TerFormatPlugin`]: `.ter` as [`Terrain`] (its heights, material map, names and alpha maps), read as a server's
//! `TerrainBlock` reads it today ([`ter::parse`]; `torque_app_server::terrain::Terrain::read` places it): files up to
//! [`MAX_TER_BYTES`], from any source (the reader is hardened and fuzzed: `torque_maps::fuzz`).

use torque_engine::assets::{LoadCx, Spec};
use torque_engine::{EngineBuilder, Plugin, PluginId};
use torque_maps::MAX_TER_BYTES;
use torque_maps::ter::{self, Terrain};

/// The terrain of a `.ter`'s bytes, as the server reads it.
pub fn read_ter(bytes: &[u8]) -> Result<Terrain, String> {
    ter::parse(bytes).map_err(|e| e.to_string())
}

/// Torque terrains: `.ter` as [`Terrain`], from any source (module docs).
#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "torque.format-ter")]
pub struct TerFormatPlugin;

impl TerFormatPlugin {
    /// What the loader takes.
    pub const SPEC: Spec = Spec::untrusted(MAX_TER_BYTES as u64);
}

impl Plugin for TerFormatPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .assets()
            .loader(&["ter"], Self::SPEC, |bytes, _: &LoadCx<'_>| {
                read_ter(bytes)
            });
    }
}
