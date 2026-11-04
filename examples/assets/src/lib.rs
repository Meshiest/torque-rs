//! The example games' shared asset kit: a unit cube, a box figure with idle, walk, run and jump sequences, a pistol, a
//! projectile, and their textures, all made by this crate's code ([`shapes`], [`textures`]; the shape builder is
//! [`builder`]) and written as `.dts` (version 24, through `torque_shapes::write`) and `.png`.
//!
//! **Made at run time, not kept in the repository.** The code is the source of the assets; an example calls [`write_all`] at
//! start-up (a few kilobytes, a millisecond), or `just example-assets <dir>` (the `make-example-assets` tool) writes them
//! once. The output is the same on every run, so nothing goes stale and no generated file sits in the history.
//!
//! Every shape is checked by the tests against torque-shapes' reader: the bytes parse back to the model that was written,
//! every mesh's bounds are the ones its vertices give, the preload finds the nodes and details a game looks up (`eye`, `cam`,
//! `mount0`, the collision and LOS details), and the figure's sequences pose its limbs the way their names say.

pub mod builder;
pub mod shapes;
pub mod textures;

use std::io;
use std::path::{Path, PathBuf};

/// One file of the kit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// The file name (no folder).
    pub name: &'static str,
    pub bytes: Vec<u8>,
}

/// Every asset of the kit, shapes first.
pub fn all() -> io::Result<Vec<Asset>> {
    let shape = |name, model| Asset {
        name,
        bytes: torque_shapes::write::write_dts(&model),
    };
    Ok(vec![
        shape("cube.dts", shapes::cube()),
        shape("figure.dts", shapes::figure()),
        shape("pistol.dts", shapes::pistol()),
        shape("projectile.dts", shapes::projectile()),
        Asset {
            name: "grid.png",
            bytes: textures::grid()?,
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
    ])
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
