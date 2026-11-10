//! The format-neutral map layer ([`torque_mapcore`]) from a map format's files, through its [`Importer`].
//!
//! [`MapLayerPlugin`] adds a loader of each part the importer makes, by the extensions of its format: a model file as
//! [`MapModel`] (its drawn surfaces, its convex hulls and the files it names), a heightfield file as
//! [`Heightfield`](torque_mapcore::collision::Heightfield) and a sky list as [`SkyList`], each file at most the importer's
//! [`Importer::max_bytes`] of its kind. The default is Torque's legacy
//! importer ([`torque_maps::legacy::IMPORTER`]: `.dif`, `.ter`, `.dml`), the one drawing and collision read through today, so
//! what a load makes is what [`Importer::surfaces`], [`Importer::convexes`], [`Importer::heightfield`] and
//! [`Importer::sky_list`] make of the same bytes. An importer never panics on any bytes and bounds its work by the file's
//! size (the trait's contract; the legacy readers are fuzzed), so the loaders take files from any source.

use std::fmt;

use torque_engine::assets::{LoadCx, Spec};
use torque_engine::{EngineBuilder, Plugin, PluginId};
use torque_mapcore::collision::ConvexSet;
use torque_mapcore::geometry::Surfaces;
use torque_mapcore::{FileKind, Importer};

/// A model file of the map layer: what draws it, what collides with it, and the files it names (its materials, as it names
/// them).
#[derive(Clone, Debug, PartialEq)]
pub struct MapModel {
    /// The drawn surfaces of its first detail level ([`Importer::surfaces`]).
    pub surfaces: Surfaces,
    /// The convex hulls of its first detail level ([`Importer::convexes`]).
    pub convexes: ConvexSet,
    /// The other files it names ([`Importer::named_files`]).
    pub named_files: Vec<String>,
}

/// A sky's material names, in order ([`Importer::sky_list`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkyList(pub Vec<String>);

/// The extensions of the files of each kind an importer reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extensions {
    /// Model files ([`FileKind::Model`]).
    pub model: &'static [&'static str],
    /// Heightfield files ([`FileKind::Heightfield`]).
    pub heightfield: &'static [&'static str],
    /// Sky lists ([`FileKind::SkyList`]).
    pub sky_list: &'static [&'static str],
}

/// The legacy importer's extensions.
pub const LEGACY_EXTENSIONS: Extensions = Extensions {
    model: &["dif"],
    heightfield: &["ter"],
    sky_list: &["dml"],
};

/// The map layer of a format's files through its importer (module docs). The default is Torque's legacy importer.
#[derive(Clone, Copy, PluginId)]
#[plugin(id = "torque.format-map-layer")]
pub struct MapLayerPlugin {
    /// The format's readers.
    pub importer: &'static dyn Importer,
    /// The extensions of its files.
    pub extensions: Extensions,
}

impl Default for MapLayerPlugin {
    fn default() -> MapLayerPlugin {
        MapLayerPlugin {
            importer: &torque_maps::legacy::IMPORTER,
            extensions: LEGACY_EXTENSIONS,
        }
    }
}

impl fmt::Debug for MapLayerPlugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MapLayerPlugin")
            .field("importer", &self.importer.name())
            .field("extensions", &self.extensions)
            .finish()
    }
}

/// The map model of a model file's bytes through `importer`.
pub fn read_model(importer: &dyn Importer, bytes: &[u8]) -> Result<MapModel, String> {
    let error = |e: torque_mapcore::ImportError| e.0;
    Ok(MapModel {
        surfaces: importer.surfaces(bytes).map_err(error)?,
        convexes: importer.convexes(bytes).map_err(error)?,
        named_files: importer
            .named_files(FileKind::Model, bytes)
            .map_err(error)?,
    })
}

impl MapLayerPlugin {
    /// What the loader of files of `kind` takes.
    pub fn spec(&self, kind: FileKind) -> Spec {
        Spec::untrusted(self.importer.max_bytes(kind) as u64)
    }
}

impl Plugin for MapLayerPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let importer = self.importer;
        let mut assets = engine.assets();
        assets.loader(
            self.extensions.model,
            self.spec(FileKind::Model),
            move |bytes, _: &LoadCx<'_>| read_model(importer, bytes),
        );
        assets.loader(
            self.extensions.heightfield,
            self.spec(FileKind::Heightfield),
            move |bytes, _: &LoadCx<'_>| importer.heightfield(bytes).map_err(|e| e.0),
        );
        assets.loader(
            self.extensions.sky_list,
            self.spec(FileKind::SkyList),
            move |bytes, _: &LoadCx<'_>| importer.sky_list(bytes).map(SkyList).map_err(|e| e.0),
        );
    }
}
