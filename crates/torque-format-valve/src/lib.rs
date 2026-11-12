//! Valve's Source formats as asset loaders: each format a plugin that adds its loader to the engine's asset registry
//! ([`torque_engine::assets`]), and [`ValveFormatPlugins`], the group of them. Not in `DefaultPlugins`: a game that reads these
//! files adds the group (or one plugin) itself.
//!
//! | Plugin | Id | Extensions | Asset | Largest file | Untrusted files |
//! |--------|----|------------|-------|--------------|-----------------|
//! | [`VtfFormatPlugin`] | `torque.format-vtf` | `vtf` | [`Texture`] | 128 MiB | yes |
//! | [`VmtFormatPlugin`] | `torque.format-vmt` | `vmt` | [`vmt::Vmt`] | 1 MiB | yes |
//! | [`BspFormatPlugin`] | `torque.format-vbsp` | `bsp` | [`bsp::Bsp`] | 512 MiB | yes |
//! | [`PcfFormatPlugin`] | `torque.format-pcf` | `pcf` | [`pcf::Pcf`] | 32 MiB | yes |
//! | [`phy::PhyFormatPlugin`] | `torque.format-phy` | `phy` | [`phy::Phy`] | 16 MiB | yes |
//! | [`MdlFormatPlugin`] | `torque.format-mdl` | `mdl`, `vvd`, `vtx`, `ani` | [`mdl::Mdl`], [`vvd::Vvd`], [`vtx::Vtx`], [`AniFile`] | 64 MiB each | yes |
//!
//! Every reader here is the crate's own, written from the formats' public documentation, and is bounds-checked: hostile bytes
//! give an error, never a panic, and every count is checked against the bytes there are before anything is allocated for it
//! (the crate's fuzz test mutates small files of every format 20,000 times). The loaders take untrusted files.
//!
//! A game's files are mounted as one tree by [`files::SourceFiles`] (its VPK archives and loose folders, as its `gameinfo.txt`
//! lists them, and a map's pak lump), a [`torque_ts_vm::host::FileHost`] the registry loads from.
//!
//! # Maintaining
//!
//! **Files on disk.** A loose file, a gameinfo, a `_dir.vpk` and a `_NNN.vpk` data file are opened once through
//! `read::open_plain` / `read::read_plain_file`, which are `torque_resource::plain` (the no-follow open flag, a check of the
//! handle, the length from the open file, at most the cap and the length the listing gave). `.vpk` files are not listed as loose files.
//!
//! **Invariants and their tests.**
//! - A listed file that grew is refused, and a read stops at its cap whatever the file says
//!   (`tests/loose_files.rs`: `a_listed_file_that_grew_is_refused_not_read`, `a_read_stops_at_the_cap_whatever_the_file_says`).
//! - Links are not followed (`links_are_not_followed`, Unix only: creating a link on Windows needs a privilege) and a folder is
//!   no file (`a_folder_is_not_a_file_and_vpk_pieces_are_not_loose_files`).
//! - The MP3 decode runs under `catch_unwind` and the fuzz seeds are real frames of each MPEG version and mode, which must decode
//!   before mutation, with header- and side-information-aware mutations (`sound::tests::the_synthetic_mp3s_decode_in_every_mpeg_version_and_mode`,
//!   `fuzz::fuzz_every_reader_without_a_panic`). The fuzz reports its slowest case and fails past a bound.
//! - `normalize_path` keeps device names: use it for table keys, never for a path on the disk.
//!
//! **Test support.** The writers that build the files the tests read (`bsp::write_test_map`, `pcf::write_test_file`,
//! `sound::write_mp3`, `vtf::write` and the like) are compiled for tests only. `sound::write_wav` and `vpk::write_dir` stay public:
//! the format viewer and `tests/loose_files.rs` call them.

pub mod brushes;
pub mod bsp;
pub mod collision;
pub mod files;
pub mod hull;
pub mod kv;
pub mod mdl;
pub mod model;
pub mod pcf;
pub mod phy;
#[cfg(feature = "collide")]
pub mod props;
pub mod read;
pub mod sound;
pub mod vmt;
pub mod vpk;
pub mod vtf;
pub mod vtx;
pub mod vvd;

use std::rc::Rc;

use torque_engine::assets::{Assets, LoadCx, Spec};
use torque_engine::{EngineBuilder, Plugin, PluginGroup, PluginGroupBuilder, PluginId};
use torque_ts_vm::host::FileHost;

pub use read::{Error, Result};

/// The largest side a texture loader decodes (larger textures load from their first smaller level).
pub const MAX_TEXTURE_SIDE: u32 = 4096;

/// A texture: its header, its RGBA8 levels (the largest first) and its sprite sheet when it has one.
#[derive(Debug, Clone, PartialEq)]
pub struct Texture {
    pub vtf: vtf::Vtf,
    pub levels: Vec<vtf::Level>,
    pub sheet: Option<pcf::Sheet>,
}

/// Reads a texture file (levels over [`MAX_TEXTURE_SIDE`] skipped; a sheet that does not read is left out).
pub fn read_texture(bytes: &[u8]) -> Result<Texture> {
    let (vtf, levels) = vtf::read(bytes, MAX_TEXTURE_SIDE)?;
    let sheet = vtf
        .resource(bytes, [0x10, 0, 0])
        .and_then(|b| pcf::Sheet::parse(b).ok());
    Ok(Texture { vtf, levels, sheet })
}

/// Textures: `.vtf` as [`Texture`], from any source.
#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "torque.format-vtf")]
pub struct VtfFormatPlugin;

impl VtfFormatPlugin {
    /// What the loader takes.
    pub const SPEC: Spec = Spec::untrusted(vtf::MAX_FILE_BYTES);
}

impl Plugin for VtfFormatPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .assets()
            .loader(&["vtf"], Self::SPEC, |bytes, _: &LoadCx<'_>| {
                read_texture(bytes).map_err(|e| e.to_string())
            });
    }
}

/// Materials: `.vmt` as [`vmt::Vmt`] (a `patch` material as written; [`vmt::Vmt::patched`] applies it), from any source.
#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "torque.format-vmt")]
pub struct VmtFormatPlugin;

impl VmtFormatPlugin {
    /// What the loader takes.
    pub const SPEC: Spec = Spec::untrusted(vmt::MAX_FILE_BYTES);
}

impl Plugin for VmtFormatPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .assets()
            .loader(&["vmt"], Self::SPEC, |bytes, _: &LoadCx<'_>| {
                vmt::Vmt::parse(bytes).map_err(|e| e.to_string())
            });
    }
}

/// An `.ani` file's bytes: the animation blocks a model's header names ([`mdl::Mdl::frame`] reads them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AniFile(pub Vec<u8>);

/// The largest `.ani` file read.
pub const MAX_ANI_BYTES: u64 = 64 << 20;

/// Studio models: `.mdl` as [`mdl::Mdl`], `.vvd` as [`vvd::Vvd`], `.vtx` as [`vtx::Vtx`] and `.ani` as [`AniFile`], from any
/// source. [`load_model`] loads the files of one model and makes them a [`model::StudioModel`].
#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "torque.format-mdl")]
pub struct MdlFormatPlugin;

impl Plugin for MdlFormatPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let mut a = engine.assets();
        a.loader(
            &["mdl"],
            Spec::untrusted(mdl::MAX_FILE_BYTES),
            |bytes, _: &LoadCx<'_>| mdl::Mdl::parse(bytes).map_err(|e| e.to_string()),
        );
        a.loader(
            &["vvd"],
            Spec::untrusted(vvd::MAX_FILE_BYTES),
            |bytes, _: &LoadCx<'_>| vvd::Vvd::parse(bytes).map_err(|e| e.to_string()),
        );
        a.loader(
            &["vtx"],
            Spec::untrusted(vtx::MAX_FILE_BYTES),
            |bytes, _: &LoadCx<'_>| vtx::Vtx::parse(bytes).map_err(|e| e.to_string()),
        );
        a.loader(
            &["ani"],
            Spec::untrusted(MAX_ANI_BYTES),
            |bytes, _: &LoadCx<'_>| Ok::<_, String>(AniFile(bytes.to_vec())),
        );
    }
}

/// The model at `path` (`models/x.mdl`) with its `.vvd` and `.vtx` (`.dx90.vtx`, else `.dx80.vtx`, else `.sw.vtx`), through
/// `assets` (the [`MdlFormatPlugin`]'s loaders): its own sequences that `keep` takes and those of the models it includes.
pub fn load_model(
    assets: &Assets,
    files: &dyn FileHost,
    path: &str,
    choice: &model::Choice,
    keep: &mut dyn FnMut(&mdl::Sequence) -> bool,
) -> Result<model::StudioModel> {
    let err = |p: &str, e: torque_engine::assets::AssetError| Error::Missing(format!("{p}: {e}"));
    let header: Rc<mdl::Mdl> = assets.load(files, path).map_err(|e| err(path, e))?;
    let stem = path.strip_suffix(".mdl").unwrap_or(path);
    let vvd_path = format!("{stem}.vvd");
    let vvd: Rc<vvd::Vvd> = assets
        .load(files, &vvd_path)
        .map_err(|e| err(&vvd_path, e))?;
    let mut vtx = None;
    let mut last = None;
    for ext in ["dx90.vtx", "dx80.vtx", "sw.vtx"] {
        let p = format!("{stem}.{ext}");
        match assets.load::<vtx::Vtx>(files, &p) {
            Ok(v) => {
                vtx = Some(v);
                break;
            }
            Err(e) => last = Some(err(&p, e)),
        }
    }
    let vtx = vtx.ok_or_else(|| last.unwrap_or(Error::Missing(format!("{stem}.dx90.vtx"))))?;
    let mut m = model::assemble(&header, &vvd, &vtx, choice)?;
    let ani = |h: &mdl::Mdl| -> Option<Rc<AniFile>> {
        if h.anim_block_file.is_empty() {
            return None;
        }
        assets.load::<AniFile>(files, &h.anim_block_file).ok()
    };
    let own = ani(&header);
    m.add_sequences(
        &header,
        &header,
        own.as_deref().map(|a| a.0.as_slice()),
        keep,
    )?;
    // Each included model once, never the model itself.
    let mut included: Vec<String> = Vec::new();
    for inc in &header.include_models {
        let key = read::normalize_path(inc).unwrap_or_default();
        if key.is_empty()
            || key == read::normalize_path(path).unwrap_or_default()
            || included.contains(&key)
        {
            continue;
        }
        included.push(key);
        let Ok(other) = assets.load::<mdl::Mdl>(files, inc) else {
            m.model
                .notes
                .push(format!("included model {inc} did not load"));
            continue;
        };
        let other_ani = ani(&other);
        m.add_sequences(
            &header,
            &other,
            other_ani.as_deref().map(|a| a.0.as_slice()),
            keep,
        )?;
    }
    Ok(m)
}

/// Maps: `.bsp` as [`bsp::Bsp`] (the header checked; each lump read when asked for), from any source.
#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "torque.format-vbsp")]
pub struct BspFormatPlugin;

impl BspFormatPlugin {
    /// What the loader takes.
    pub const SPEC: Spec = Spec::untrusted(bsp::MAX_FILE_BYTES);
}

impl Plugin for BspFormatPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .assets()
            .loader(&["bsp"], Self::SPEC, |bytes, _: &LoadCx<'_>| {
                bsp::Bsp::parse(bytes).map_err(|e| e.to_string())
            });
    }
}

/// Particle systems: `.pcf` as [`pcf::Pcf`] (binary DMX, encoding 2), from any source.
#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "torque.format-pcf")]
pub struct PcfFormatPlugin;

impl PcfFormatPlugin {
    /// What the loader takes.
    pub const SPEC: Spec = Spec::untrusted(pcf::MAX_FILE_BYTES);
}

impl Plugin for PcfFormatPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .assets()
            .loader(&["pcf"], Self::SPEC, |bytes, _: &LoadCx<'_>| {
                pcf::Pcf::parse(bytes).map_err(|e| e.to_string())
            });
    }
}

/// Every Source format (crate docs).
#[derive(Clone, Copy, Debug, Default)]
pub struct ValveFormatPlugins;

impl PluginGroup for ValveFormatPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::new()
            .add(VtfFormatPlugin)
            .add(VmtFormatPlugin)
            .add(MdlFormatPlugin)
            .add(BspFormatPlugin)
            .add(PcfFormatPlugin)
            .add(phy::PhyFormatPlugin)
    }
}

#[cfg(test)]
mod fuzz;
#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod phy_tests;
#[cfg(test)]
mod synth;
#[cfg(test)]
mod tests;
