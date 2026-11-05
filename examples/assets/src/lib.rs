//! The example games' shared asset kit: a unit cube, a box figure with idle, walk, run, back, side, jump, look, death and
//! sit sequences, a pistol, a projectile, a dune buggy and its wheel, a terrain of hills with a race track, the arena's
//! interior, forests of low-poly trees, their textures and a particle's, all made by this crate's code ([`shapes`], [`buggy`],
//! [`terrain`], [`track`], [`interiors`], [`trees`], [`textures`]; the shape
//! builder is the `torque-shape-builder` crate, [`builder`], the interior builder `torque-interior-builder`) and written as
//! `.dts` (version 24, through `torque_shapes::write`), `.ter`, `.dif` (version 0, through `torque_maps::dif::write`) and
//! `.png`. The valley ([`valley`]: a terrain with a lake and woods, and its ground's textures) is made only by a game that asks
//! for it ([`valley::files`]).
//!
//! **Made at run time, not kept in the repository.** The code is the source of the assets; an example calls [`write_all`] at
//! start-up (a few kilobytes, a millisecond), or `just examples/assets <dir>` (the `make-example-assets` tool) writes them
//! once. The output is the same on every run, so nothing goes stale and no generated file sits in the history.
//!
//! Every shape is checked by the tests against torque-shapes' reader: the bytes parse back to the model that was written,
//! every mesh's bounds are the ones its vertices give, the preload finds the nodes and details a game looks up (`eye`, `cam`,
//! `mount0`, the collision and LOS details), and the figure's sequences pose its limbs the way their names say.

pub use torque_shape_builder as builder;
pub mod buggy;
pub mod gait;
pub mod interiors;
pub mod shapes;
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
    pub name: &'static str,
    pub bytes: Vec<u8>,
}

/// The shape `model` as a `.glb` (through the model layer): read back, it makes the same `.dts`.
pub fn glb_of(model: &torque_shapes::Model) -> io::Result<Vec<u8>> {
    let layer = torque_import::dts::from_dts(model).map_err(io::Error::other)?;
    torque_import::gltf::write_glb(&layer).map_err(io::Error::other)
}

/// Every asset of the kit, shapes first (the cube, the figure and the pistol also as `.glb`).
pub fn all() -> io::Result<Vec<Asset>> {
    let shape = |name, model: &torque_shapes::Model| Asset {
        name,
        bytes: torque_shapes::write::write_dts(model),
    };
    let glb = |name, model: &torque_shapes::Model| -> io::Result<Asset> {
        Ok(Asset {
            name,
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
            name: "hills.ter",
            bytes: terrain::hills(),
        },
        Asset {
            name: "arena.dif",
            bytes: interiors::arena_dif()?,
        },
        Asset {
            name: "keep.dif",
            bytes: interiors::keep_dif()?,
        },
        Asset {
            name: "yard.dif",
            bytes: interiors::yard_dif()?,
        },
        Asset {
            name: "grid.png",
            bytes: textures::grid()?,
        },
        Asset {
            name: "arena_floor.png",
            bytes: textures::grid_tinted(textures::SAND)?,
        },
        Asset {
            name: "arena_wall.png",
            bytes: textures::grid_tinted(textures::SLATE)?,
        },
        Asset {
            name: "figure.png",
            bytes: textures::figure()?,
        },
        Asset {
            name: "pistol.png",
            bytes: textures::pistol()?,
        },
        Asset {
            name: "projectile.png",
            bytes: textures::projectile()?,
        },
        Asset {
            name: "spark.png",
            bytes: textures::spark()?,
        },
        Asset {
            name: "hole.png",
            bytes: textures::hole()?,
        },
        Asset {
            name: "hole2.png",
            bytes: textures::hole_of(2)?,
        },
        Asset {
            name: "hole3.png",
            bytes: textures::hole_of(3)?,
        },
        Asset {
            name: "flare_star.png",
            bytes: textures::flare_star()?,
        },
        Asset {
            name: "smoke.png",
            bytes: textures::smoke()?,
        },
        Asset {
            name: "checker.png",
            bytes: textures::checker()?,
        },
        Asset {
            name: "buggy.png",
            bytes: textures::buggy()?,
        },
        Asset {
            name: "track.png",
            bytes: textures::track()?,
        },
        Asset {
            name: "dirt.png",
            bytes: textures::dirt()?,
        },
        Asset {
            name: "tree_bark.png",
            bytes: textures::bark()?,
        },
        Asset {
            name: "tree_bark_red.png",
            bytes: textures::red_bark()?,
        },
        Asset {
            name: "tree_bark_birch.png",
            bytes: textures::birch_bark()?,
        },
        Asset {
            name: "tree_pine.png",
            bytes: textures::foliage(textures::PINE, 101)?,
        },
        Asset {
            name: "tree_fir.png",
            bytes: textures::foliage(textures::FIR, 113)?,
        },
        Asset {
            name: "tree_leaves.png",
            bytes: textures::foliage(textures::LEAVES, 131)?,
        },
        Asset {
            name: "tree_leaves_dark.png",
            bytes: textures::foliage(textures::LEAVES_DARK, 141)?,
        },
        Asset {
            name: "tree_leaves_light.png",
            bytes: textures::foliage(textures::LEAVES_LIGHT, 151)?,
        },
        Asset {
            name: trees::LINEUP_FILE,
            bytes: trees::lineup_dif()?,
        },
    ];
    out.extend(
        trees::chunk_difs()?
            .into_iter()
            .map(|(name, bytes)| Asset { name, bytes }),
    );
    Ok(out)
}

/// Writes every asset into `dir` (made if missing; files already there are replaced) and returns the paths written.
pub fn write_all(dir: &Path) -> io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    all()?
        .into_iter()
        .map(|a| {
            let path = dir.join(a.name);
            std::fs::write(&path, &a.bytes)?;
            Ok(path)
        })
        .collect()
}

#[cfg(test)]
mod tests;
