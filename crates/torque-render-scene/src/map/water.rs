//! A map's water (`WaterBlock`): the top of the block's box drawn as a water plane (a blended plane after the opaque
//! objects), with the block's surface texture at its `surfaceOpacity`. A surface texture found nowhere is looked for in the
//! profile's water folder by its file name, else the profile's water texture is drawn ([`super::MapProfile`]).
//!
//! The box is the object's unit box `(0, 0, 0)..(1, 1, 1)` scaled and placed by its transform (TGE's fluid sits at its box's
//! top). The top is cut into cells of at most [`CELL_METRES`] (at most [`MAX_CELLS`] a side) so that fog and lighting are
//! interpolated over it; the texture repeats the profile's `water_loops_per_unit` times a unit in world x and y. GL lighting with
//! an up normal, both sides drawn, no depth write.
//!
//! Credit: follows TGE 1.4's `terrain/waterBlock.cc` and `terrain/fluidRender.cc` (Copyright (C) GarageGames.com, Inc.); the
//! code here is this crate's own.

use std::sync::Arc;

use super::{Fnv, Mesh, Object, TexRef, find_texture, folder_of};
use super::{MapClass, MapProfile};
use crate::textures::TextureSet;
use crate::{Blend, Cull, TexEnv, Vertex};

/// The largest cell of a water surface's grid.
pub const CELL_METRES: f32 = 32.0;
/// The most cells on a side.
pub const MAX_CELLS: usize = 64;

/// The start of every water mesh's label.
pub const LABEL: &str = "water";

pub(crate) fn meshes(
    objects: &[Object],
    profile: &MapProfile,
    textures: &mut TextureSet<'_>,
    key: u64,
) -> Vec<Mesh> {
    let loops = profile.water_loops_per_unit;
    let mut out = Vec::new();
    for (n, o) in objects
        .iter()
        .filter(|o| o.class == MapClass::WaterBlock)
        .enumerate()
    {
        let m = o.matrix();
        let s = o.scale();
        let name = o.text("surfaceTexture");
        let found = find_texture(textures, &name, &[folder_of(&name), profile.water_fallback])
            .or_else(|| find_texture(textures, profile.water_texture, &[]));
        let texture = match found {
            Some((_, found)) => TexRef::Name(found),
            None => TexRef::Missing(name.clone()),
        };
        let opacity = o.f32("surfaceOpacity").clamp(0.0, 1.0);
        let alpha = (opacity * 255.0).round() as u8;
        let cells = |len: f32| ((len.abs() / CELL_METRES).ceil() as usize).clamp(1, MAX_CELLS);
        let (nx, ny) = (cells(s[0]), cells(s[1]));
        let point = |i: usize, j: usize| {
            let p = m.mul_p([
                s[0] * i as f32 / nx as f32,
                s[1] * j as f32 / ny as f32,
                s[2],
            ]);
            Vertex {
                position: p,
                normal: [0.0, 0.0, 1.0],
                color: [255, 255, 255, alpha],
                uv: [p[0] * loops, p[1] * loops],
            }
        };
        let mut vertices = Vec::with_capacity(nx * ny * 6);
        for j in 0..ny {
            for i in 0..nx {
                let q = [
                    point(i, j),
                    point(i, j + 1),
                    point(i + 1, j + 1),
                    point(i + 1, j),
                ];
                vertices.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
            }
        }
        if !vertices
            .iter()
            .all(|v| v.position.iter().all(|c| c.is_finite()))
        {
            continue;
        }
        let centre = m.mul_p([s[0] * 0.5, s[1] * 0.5, s[2]]);
        let mut h = Fnv::new("map water");
        h.u64(key);
        h.u64(n as u64);
        out.push(Mesh {
            label: format!("{LABEL} {name}"),
            texture,
            vertices: Arc::new(vertices),
            lighting: true,
            tex_env: TexEnv::Modulate,
            blend: Blend::Alpha,
            cull: Cull::None,
            depth_write: false,
            sampler: super::SAMPLER,
            alpha_from_texture: false,
            overlay: false,
            centre,
            key: h.0,
        });
    }
    out
}
