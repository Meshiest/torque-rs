//! Content formats as asset loaders: each format a plugin that adds its loaders to the engine's asset registry
//! ([`torque_engine::assets`]), and [`ContentFormatPlugins`], the group of them.
//!
//! | Plugin | Id | Extensions | Asset | Largest file | Untrusted files | In the group |
//! |--------|----|------------|-------|--------------|-----------------|--------------|
//! | [`GltfFormatPlugin`] | `torque.format-gltf` | `glb`, `gltf` | [`ImportedShape`] | 16 MiB | yes | on (`gltf` feature) |
//! | [`ObjFormatPlugin`] | `torque.format-obj` | `obj` | [`ImportedShape`] | 64 MiB | no | off |
//! | [`BlockbenchFormatPlugin`] | `torque.format-blockbench` | `bbmodel` | [`ImportedShape`] | 64 MiB | no | off (`gltf` feature) |
//! | [`DifFormatPlugin`] | `torque.format-dif` | `dif` | [`torque_maps::dif::Dif`] | 64 MiB | yes | on |
//! | [`TerFormatPlugin`] | `torque.format-ter` | `ter` | [`torque_maps::ter::Terrain`] | 16 MiB | yes | on |
//! | [`MapLayerPlugin`] | `torque.format-map-layer` | `dif`, `ter`, `dml` | [`MapModel`], [`Heightfield`](torque_mapcore::collision::Heightfield), [`SkyList`] | the importer's | yes | on |
//! | `QuakeMapFormatPlugin` | `torque.format-quake-map` | `map` | `BuiltInterior` | 64 MiB | no | off (`map-builder` feature) |
//!
//! **Trust.** A loader takes files from untrusted sources (what a server sends, a mod) only when its reader is hardened against
//! hostile bytes and fuzzed today: the glTF reader (its fuzz makes mutated `.glb`s into shapes) and Torque's `.dif`, `.ter` and
//! `.dml` readers (`torque_maps::fuzz`). The OBJ and Blockbench readers' fuzz stops at the model and never makes the shape a
//! load makes; the `.map` reader caps what it reads but is not fuzzed, and building an interior is a long piece of work. Those
//! three take trusted files only.
//!
//! **What the engine loads today.** TGE's shapes, interiors and terrains still load the way they always have (a datablock's
//! shape file, an `InteriorInstance`'s `interiorFile`, a `TerrainBlock`'s `terrainFile`, each paid for from the script
//! engine's budget and kept in that path's own cache). These loaders sit beside those paths and read the same bytes into the
//! same results, for a plugin or a game that loads a file itself (`engine.assets(&mut vm).load::<ImportedShape>(...)`).
//!
//! **In a group.** [`ContentFormatPlugins`] has every format: the ones the engine reads today on (glTF, `.dif`, `.ter`, the map
//! layer), the ones only the converter reads today off (OBJ, Blockbench, `.map`). `DefaultPlugins` and `MinimalPlugins` have the
//! group. A game drops a format with `DefaultPlugins.disable::<DifFormatPlugin>()` or its `game.toml`
//! (`[plugins] disable = ["torque.format-dif"]`) and turns one of the others on with `enable`.
//!
//! ```
//! use torque_content::{ContentFormatPlugins, ImportedShape, ObjFormatPlugin};
//! use torque_engine::{EngineBuilder, PluginGroup};
//! use torque_ts_vm::host::MemFiles;
//! use torque_ts_vm::vm::Vm;
//!
//! let engine = EngineBuilder::new()
//!     .add_plugins(ContentFormatPlugins.enable::<ObjFormatPlugin>())
//!     .build()?;
//! let files = MemFiles::new();
//! files.insert("art/tri.obj", b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n");
//! let mut vm = Vm::new();
//! vm.set_file_host(files);
//! let assets = engine.assets(&mut vm);
//! let shape = assets.load::<ImportedShape>(vm.file_host(), "art/tri.obj")?;
//! assert!(!shape.dts.is_empty());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod interiors;
pub mod map_layer;
pub mod shapes;
pub mod terrain;

use torque_engine::{PluginGroup, PluginGroupBuilder};

pub use interiors::DifFormatPlugin;
#[cfg(feature = "map-builder")]
pub use interiors::{BuiltInterior, QuakeMapFormatPlugin};
pub use map_layer::{MapLayerPlugin, MapModel, SkyList};
#[cfg(feature = "gltf")]
pub use shapes::{BlockbenchFormatPlugin, GltfFormatPlugin};
pub use shapes::{ImportedShape, ObjFormatPlugin};
pub use terrain::TerFormatPlugin;

/// The most work units one model's import is paid ([`torque_import::Charge`]: about one a byte read and one an element made);
/// past it the load is an error.
pub const MAX_IMPORT_WORK: u64 = 1 << 28;

/// A work budget of `units`, paid down by each charge; it refuses the charge that would take it below zero.
pub(crate) fn budget(units: u64) -> impl FnMut(u64) -> bool {
    let mut left = units;
    move |cost| match left.checked_sub(cost) {
        Some(rest) => {
            left = rest;
            true
        }
        None => false,
    }
}

/// Every content format (module docs): the ones the engine reads today on, the others added off for a game to turn on.
#[derive(Clone, Copy, Debug, Default)]
pub struct ContentFormatPlugins;

impl PluginGroup for ContentFormatPlugins {
    fn build(self) -> PluginGroupBuilder {
        let group = PluginGroupBuilder::new();
        #[cfg(feature = "gltf")]
        let group = group.add(GltfFormatPlugin);
        let group = group.add(ObjFormatPlugin);
        #[cfg(feature = "gltf")]
        let group = group.add(BlockbenchFormatPlugin);
        let group = group
            .add(DifFormatPlugin)
            .add(TerFormatPlugin)
            .add(MapLayerPlugin::default());
        #[cfg(feature = "map-builder")]
        let group = group.add(QuakeMapFormatPlugin::default());
        let group = group.disable::<ObjFormatPlugin>();
        #[cfg(feature = "gltf")]
        let group = group.disable::<BlockbenchFormatPlugin>();
        #[cfg(feature = "map-builder")]
        let group = group.disable::<QuakeMapFormatPlugin>();
        group
    }
}

#[cfg(test)]
mod tests;
