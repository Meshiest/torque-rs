//! What a node's name says, by Torque's naming conventions: a mesh's detail level from the number its name ends with
//! (`box2`, `Col-1`, `LOSCol-9`), collision and line-of-sight meshes (and `col_*` / `hull_*` authored hulls), mission volumes
//! (`water*`, `trigger*`, `physical_zone*`) and the points a game looks up (`mount0`..`mount31`, `eye`, `cam`, `muzzlePoint`,
//! `ejectPoint`).
//!
//! Credit: [`trailing_number`] and [`mesh_detail`] follow `String::GetTrailingNumber` and the detail naming of
//! `TSShapeLoader::generateObjects` and `TSShape::addDetail` in Torque3D (`Engine/source/core/util/str.cpp`,
//! `Engine/source/ts/loader/tsShapeLoader.cpp`, `Engine/source/ts/tsShapeEdit.cpp`: Copyright (c) 2012 GarageGames, LLC;
//! MIT). The code here is this crate's own.

/// The detail size of a mesh whose name ends with no number.
pub const DEFAULT_DETAIL_SIZE: i32 = 2;

/// A name without the number it ends with (and the spaces before it), and that number: negative after a `-` or `_`.
pub fn trailing_number(name: &str) -> (&str, Option<i32>) {
    let trimmed = name.trim_end();
    let digits = trimmed.bytes().rev().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 9 {
        return (name, None);
    }
    let at = trimmed.len() - digits;
    let Ok(n) = trimmed[at..].parse::<i32>() else {
        return (name, None);
    };
    let (base, n) = match trimmed[..at].strip_suffix(['-', '_']) {
        Some(b) => (b, -n),
        None => (&trimmed[..at], n),
    };
    (base.trim_end(), Some(n))
}

/// Where a mesh named `name` goes: its object's name, its detail's name and size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshDetail {
    pub object: String,
    pub detail: String,
    pub size: i32,
}

fn starts(name: &str, prefix: &str) -> bool {
    name.get(..prefix.len())
        .is_some_and(|h| h.eq_ignore_ascii_case(prefix))
}

/// The object and detail of a mesh named `name`: the number it ends with is the detail size ([`DEFAULT_DETAIL_SIZE`] when
/// none; a `Collision*` or `LOSCol*` name always takes a negative one). A negative size goes to the `Collision` detail of that
/// size for a `Col*` name, to the `LOS` detail for a `LOS*` name, and any other mesh to `detail<size>`.
pub fn mesh_detail(name: &str) -> MeshDetail {
    let (base, n) = trailing_number(name);
    let mut size = n.unwrap_or(DEFAULT_DETAIL_SIZE);
    if (starts(base, "collision") || starts(base, "loscol")) && size > 0 {
        size = -size;
    }
    let collision = starts(base, "col");
    let los = starts(base, "los");
    let prefix = if size < 0 && collision {
        "Collision"
    } else if size < 0 && los {
        "LOS"
    } else {
        "detail"
    };
    MeshDetail {
        object: if base.is_empty() {
            name.to_string()
        } else {
            base.to_string()
        },
        detail: format!("{prefix}{size}"),
        size,
    }
}

/// A mission volume an interior's input may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Volume {
    Water,
    Trigger,
    PhysicalZone,
}

/// What a mesh node is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Drawn.
    Visible,
    /// A collision mesh (`Col*`, `Collision*` with a negative number, `col_*`, `hull_*`).
    Collision,
    /// A line-of-sight mesh (`LOS*` with a negative number).
    Los,
    /// A mission volume (its box), not drawn.
    Volume(Volume),
}

/// The role of a mesh node named `name` (case ignored).
pub fn role(name: &str) -> Role {
    if starts(name, "col_") || starts(name, "hull_") {
        return Role::Collision;
    }
    if starts(name, "water") {
        return Role::Volume(Volume::Water);
    }
    if starts(name, "trigger") {
        return Role::Volume(Volume::Trigger);
    }
    if starts(name, "physical_zone") {
        return Role::Volume(Volume::PhysicalZone);
    }
    let d = mesh_detail(name);
    if d.size < 0 && starts(&d.detail, "collision") {
        Role::Collision
    } else if d.size < 0 && starts(&d.detail, "los") {
        Role::Los
    } else {
        Role::Visible
    }
}

/// Whether `name` is a point a game looks up on a shape (mount points, the eye and camera, the muzzle and eject points).
pub fn is_named_point(name: &str) -> bool {
    let mount = starts(name, "mount")
        && name[5..].bytes().all(|b| b.is_ascii_digit())
        && name[5..].parse::<u32>().is_ok_and(|n| n < 32);
    mount
        || ["eye", "cam", "muzzlePoint", "ejectPoint", "mountPoint"]
            .iter()
            .any(|p| name.eq_ignore_ascii_case(p))
}
