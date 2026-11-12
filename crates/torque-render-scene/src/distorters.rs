//! The shapes and decals whose datablocks bend the picture behind them (the engine protocol's distortion block,
//! [`ghosts::datablocks::distortion::DistortionExt`]), as the frame's [`HdDistorter`]s.
//!
//! - [`DistortionData::read`]: a datablock's fields (`distortion`, `distortionEdge`, `distortionMap`, ...), sanitized again (the
//!   world can hold a datablock the wire's reader did not make); None when `distortion` is 0 (a server on Torque's protocol
//!   sends none).
//! - [`shape_distorters`]: each moving object whose ghost's datablock bends is a distorter too, made of its draws as they are
//!   placed this frame; one drawn as the bend alone (`distortionOnly`: a heat haze, a shockwave's sphere) leaves the frame's
//!   objects, so it is neither drawn nor cast into a shadow.
//! - A decal's (`DecalData`) bends as its triangles do, fading with it ([`crate::decals::DecalManager::distorters`]).

use ghosts::Value;
use ghosts::codec::F32;
use ghosts::datablocks::distortion::DistortionExt;

use crate::hd::DynamicObject;
use crate::hd::distortion::{DistortionBudget, DistortionLook, HdDistorter, nearest_first};
use crate::textures::TextureSet;
use crate::world::ShapeWorld;
use crate::{Filter, Sampler};

/// A datablock's distortion: its look but the map, the map's texture name, and whether it is drawn as the bend alone.
#[derive(Debug, Clone, PartialEq)]
pub struct DistortionData {
    pub look: DistortionLook,
    pub map: Option<String>,
    pub only: bool,
}

/// The sampler a distortion map is read with: repeated, filtered, mipmapped.
const MAP_SAMPLER: Sampler = Sampler {
    repeat: true,
    mag: Filter::Linear,
    min: Filter::Linear,
    mip: Filter::Linear,
    mipmaps: true,
};

impl DistortionData {
    /// The distortion the fields `field(name, element)` give; None when it changes nothing.
    pub fn read<'v>(field: impl Fn(&str, u32) -> Option<&'v Value>) -> Option<DistortionData> {
        let d = DistortionExt::default();
        let float = |name: &str, i: u32, default: F32| match field(name, i) {
            Some(Value::F32(x)) => F32::new(*x),
            _ => default,
        };
        let e = DistortionExt {
            strength: float("distortion", 0, d.strength),
            edge: float("distortionEdge", 0, d.edge),
            // (The map's name is read below, once it bends.)
            map: Vec::new(),
            map_scale: float("distortionMapScale", 0, d.map_scale),
            scroll: [0, 1].map(|i| float("distortionScroll", i, d.scroll[i as usize])),
            tint: [0, 1, 2, 3].map(|i| float("distortionTint", i, d.tint[i as usize])),
            chroma: float("distortionChroma", 0, d.chroma),
            only: matches!(field("distortionOnly", 0), Some(Value::Bool(true))),
        }
        .sanitize();
        if !e.bends() {
            return None;
        }
        let map = match field("distortionMap", 0) {
            Some(Value::Str(s)) => String::from_utf8_lossy(s).replace('\\', "/"),
            _ => String::new(),
        };
        Some(DistortionData {
            look: DistortionLook {
                strength: e.strength.get(),
                edge: e.edge.get(),
                map: None,
                map_scale: e.map_scale.get(),
                scroll: e.scroll.map(F32::get),
                tint: e.tint.map(F32::get),
                chroma: e.chroma.get(),
            },
            map: (!map.is_empty()).then_some(map),
            only: e.only,
        })
    }

    /// The look with its map found in `textures` (its index into them; none when the map is not found).
    pub fn look(&self, textures: &mut TextureSet<'_>) -> DistortionLook {
        DistortionLook {
            map: self
                .map
                .as_deref()
                .and_then(|m| textures.get(m))
                .map(|t| (t, MAP_SAMPLER)),
            ..self.look
        }
    }
}

/// The distorters of `objects` whose ghosts' datablocks in `world` bend (their maps' indices into `textures`): those `sees`
/// may show (a sphere's centre and radius), the nearest to `eye` first within a [`DistortionBudget`] (a server's ghosts decide
/// how many bend). The objects drawn as the bend alone are taken out of `objects`, kept as distorters or not.
pub fn shape_distorters<W: ShapeWorld + ?Sized>(
    world: &W,
    objects: &mut Vec<DynamicObject>,
    eye: [f32; 3],
    sees: impl Fn([f32; 3], f32) -> bool,
    textures: &mut TextureSet<'_>,
) -> Vec<HdDistorter> {
    let records = world.records();
    let bends: Vec<Option<DistortionData>> = objects
        .iter()
        .map(|o| {
            let db = records.ghosts.get(&o.ghost)?.datablock()?;
            DistortionData::read(|name, i| world.field(db, name, i))
        })
        .collect();
    let candidates: Vec<usize> = (0..objects.len())
        .filter(|&i| bends[i].is_some() && sees(objects[i].centre, objects[i].radius))
        .collect();
    let order = nearest_first(
        eye,
        candidates
            .iter()
            .map(|&i| (objects[i].centre, objects[i].radius)),
    );
    let mut budget = DistortionBudget::default();
    let mut out = Vec::new();
    for k in order {
        if budget.is_spent() {
            break;
        }
        let i = candidates[k];
        let (o, Some(data)) = (&objects[i], &bends[i]) else {
            continue;
        };
        let drawn = o.draws.iter().filter(|d| !d.indices.is_empty());
        let (count, vertices, indices) = drawn.fold((0, 0, 0), |(n, v, x), d| {
            (n + 1, v + d.vertices.len(), x + d.indices.len())
        });
        if budget.take(count, vertices, indices) {
            out.extend(HdDistorter::from_object(o, data.look(textures)));
        }
    }
    let mut only = bends.iter().map(|b| b.as_ref().is_some_and(|d| d.only));
    objects.retain(|_| !only.next().unwrap_or(false));
    out
}

#[cfg(test)]
#[path = "distorters_tests.rs"]
mod tests;
