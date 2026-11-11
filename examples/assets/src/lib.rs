//! The example games' shared asset kit: a unit cube, a box figure with idle, walk, run, back, side, jump, look, death, sit,
//! crouched, prone, swimming and hit-reaction sequences, a pistol, a projectile, a dune buggy and its wheel, a terrain of hills with a race track, the arena's
//! interior, forests of low-poly trees, their textures and a particle's, all made by this crate's code ([`shapes`], [`poses`], [`swim`], [`buggy`],
//! [`terrain`], [`track`], [`interiors`], [`trees`], [`textures`]; the shape
//! builder is the `torque-shape-builder` crate, [`builder`], the interior builder `torque-interior-builder`) and written as
//! `.dts` (version 24, through `torque_shapes::write`), `.ter`, `.dif` (version 0, through `torque_maps::dif::write`) and
//! `.png`. The valley ([`valley`]: a terrain with a lake and woods, and its ground's textures) is made only by a game that asks
//! for it ([`valley::files`]).
//!
//! **Made at run time, not kept in the repository.** The code is the source of the assets; an example calls [`all`] at
//! start-up, or `just examples assets write <dir>` (the `make-example-assets` tool) writes them once. The output is the same
//! on every run, so nothing goes stale and no generated file sits in the history.
//!
//! Making the kit takes half a second to a second and a half (the interiors' CSG, BSP and lightmaps, the forest's squares),
//! so in a development build [`all`] keeps it under the build folder ([`cache`]: `<target>/example-kit/<key>/`, the key a
//! digest of the code that makes it) and reads it back; [`make`] always makes it afresh. `TORQUE_KIT_CACHE` names another
//! folder (any build) or turns the cache off (`0`).
//!
//! Every shape is checked by the tests against torque-shapes' reader: the bytes parse back to the model that was written,
//! every mesh's bounds are the ones its vertices give, the preload finds the nodes and details a game looks up (`eye`, `cam`,
//! `mount0`, the collision and LOS details), and the figure's sequences pose its limbs the way their names say.

pub use torque_shape_builder as builder;
pub mod buggy;
pub mod cache;
pub mod gait;
pub mod interiors;
pub mod poses;
pub mod shapes;
pub mod swim;
pub mod terrain;
pub mod textures;
pub mod track;
pub mod trees;
pub mod valley;

use std::io;
use std::path::{Path, PathBuf};

/// One file of the kit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// The file name (no folder).
    pub name: String,
    pub bytes: Vec<u8>,
}

/// The shape `model` as a `.glb` (through the model layer): read back, it makes the same `.dts`.
pub fn glb_of(model: &torque_shapes::Model) -> io::Result<Vec<u8>> {
    let layer = torque_import::dts::from_dts(model).map_err(io::Error::other)?;
    torque_import::gltf::write_glb(&layer).map_err(io::Error::other)
}

/// Every asset of the kit, shapes first (the cube, the figure and the pistol also as `.glb`): read from the [`cache`] when
/// it holds the kit of this code, else made ([`make`]) and kept there.
pub fn all() -> io::Result<Vec<Asset>> {
    cache::read_or_make()
}

/// Every asset of the kit made afresh from its code, in [`all`]'s order.
pub fn make() -> io::Result<Vec<Asset>> {
    let shape = |name: &str, model: &torque_shapes::Model| Asset {
        name: name.into(),
        bytes: torque_shapes::write::write_dts(model),
    };
    let glb = |name: &str, model: &torque_shapes::Model| -> io::Result<Asset> {
        Ok(Asset {
            name: name.into(),
            bytes: glb_of(model)?,
        })
    };
    let (cube, figure, pistol) = (shapes::cube(), shapes::figure(), shapes::pistol());
    let mut out = vec![
        shape("cube.dts", &cube),
        shape("figure.dts", &figure),
        shape("pistol.dts", &pistol),
        glb("cube.glb", &cube)?,
        glb("figure.glb", &figure)?,
        glb("pistol.glb", &pistol)?,
        shape("projectile.dts", &shapes::projectile()),
        shape("buggy.dts", &buggy::buggy()),
        shape("buggy_wheel.dts", &buggy::wheel()),
        Asset {
            name: "hills.ter".into(),
            bytes: terrain::hills(),
        },
        Asset {
            name: "arena.dif".into(),
            bytes: interiors::arena_dif()?,
        },
        Asset {
            name: "keep.dif".into(),
            bytes: interiors::keep_dif()?,
        },
        Asset {
            name: "yard.dif".into(),
            bytes: interiors::yard_dif()?,
        },
        Asset {
            name: "grid.png".into(),
            bytes: textures::grid()?,
        },
        Asset {
            name: "arena_floor.png".into(),
            bytes: textures::grid_tinted(textures::SAND)?,
        },
        Asset {
            name: "arena_wall.png".into(),
            bytes: textures::grid_tinted(textures::SLATE)?,
        },
        Asset {
            name: "figure.png".into(),
            bytes: textures::figure()?,
        },
        Asset {
            name: "pistol.png".into(),
            bytes: textures::pistol()?,
        },
        Asset {
            name: "projectile.png".into(),
            bytes: textures::projectile()?,
        },
        Asset {
            name: "spark.png".into(),
            bytes: textures::spark()?,
        },
        Asset {
            name: "hole.png".into(),
            bytes: textures::hole()?,
        },
        Asset {
            name: "hole2.png".into(),
            bytes: textures::hole_of(2)?,
        },
        Asset {
            name: "hole3.png".into(),
            bytes: textures::hole_of(3)?,
        },
        Asset {
            name: "flare_star.png".into(),
            bytes: textures::flare_star()?,
        },
        Asset {
            name: "smoke.png".into(),
            bytes: textures::smoke()?,
        },
        Asset {
            name: "checker.png".into(),
            bytes: textures::checker()?,
        },
        Asset {
            name: "buggy.png".into(),
            bytes: textures::buggy()?,
        },
        Asset {
            name: "track.png".into(),
            bytes: textures::track()?,
        },
        Asset {
            name: "dirt.png".into(),
            bytes: textures::dirt()?,
        },
        Asset {
            name: "tree_bark.png".into(),
            bytes: textures::bark()?,
        },
        Asset {
            name: "tree_bark_red.png".into(),
            bytes: textures::red_bark()?,
        },
        Asset {
            name: "tree_bark_birch.png".into(),
            bytes: textures::birch_bark()?,
        },
        Asset {
            name: "tree_pine.png".into(),
            bytes: textures::foliage(textures::PINE, 101)?,
        },
        Asset {
            name: "tree_fir.png".into(),
            bytes: textures::foliage(textures::FIR, 113)?,
        },
        Asset {
            name: "tree_leaves.png".into(),
            bytes: textures::foliage(textures::LEAVES, 131)?,
        },
        Asset {
            name: "tree_leaves_dark.png".into(),
            bytes: textures::foliage(textures::LEAVES_DARK, 141)?,
        },
        Asset {
            name: "tree_leaves_light.png".into(),
            bytes: textures::foliage(textures::LEAVES_LIGHT, 151)?,
        },
        Asset {
            name: trees::LINEUP_FILE.into(),
            bytes: trees::lineup_dif()?,
        },
    ];
    out.extend(trees::chunk_difs()?.into_iter().map(|(name, bytes)| Asset {
        name: name.into(),
        bytes,
    }));
    Ok(out)
}

/// Writes every asset into `dir` (made if missing; files already there are replaced) and returns the paths written.
pub fn write_all(dir: &Path) -> io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    all()?
        .into_iter()
        .map(|a| {
            let path = dir.join(&a.name);
            std::fs::write(&path, &a.bytes)?;
            Ok(path)
        })
        .collect()
}

#[cfg(test)]
mod tests;
