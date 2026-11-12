//! The particles of the `ParticleEmitterNode`s a server ghosts (a map's fountains, chimneys and torches), run by the client
//! (TGE's `ParticleEmitterNode::advanceTime`): each node with an emitter datablock emits at its place along its z axis, with
//! the velocity `velocity` (the field's default; the ghost does not carry it) along it, for `dt * timeMultiple` of the
//! emitter's clock, `timeMultiple` being its `ParticleEmitterNodeData`'s.
//!
//! - **Time**: the milliseconds a frame owes run whole; the rest waits for the next frame, so a node makes
//!   `1000 / ejectionPeriodMS` particles a second whatever the frame rate (TGE cuts the fraction off each frame).
//! - **Caps**: [`MAX_NODES`] emitters, one a node, in ghost order; the particle engine's own ([`crate::particles::MAX_PARTICLES`])
//!   for all of them. A node whose place, scale or axis is not finite emits nothing; one whose emitter datablock changes (or
//!   whose datablocks the server sent again) starts a new emitter.
//!
//! Credit: follows TGE 1.4's `game/fx/particleEmitter.cc` (`ParticleEmitterNode::advanceTime`; Copyright (C) GarageGames.com,
//! Inc.), as Torque3D carries it under the MIT license in `T3D/fx/particleEmitterNode.cpp` (Copyright (c) 2012 GarageGames, LLC).
//! The code here is this crate's own.

use std::collections::BTreeMap;

use ghost_world::tge::TgeClientSet;
use ghost_world::world::{ClientWorld, Pose};
use ghosts::Value;
use ghosts::tge::{TgeClass, TgeRecord};

use crate::Draw;
use crate::math::Mat;
use crate::particles::{Emitter, Rng, SpriteFrame};
use crate::textures::TextureSet;

/// The most nodes that emit.
pub const MAX_NODES: usize = 128;
/// The velocity a node emits with along its axis (`ParticleEmitterNode::velocity`'s default).
pub const NODE_VELOCITY: f32 = 1.0;
/// The range of `timeMultiple` a node's data may ask (TGE refuses a datablock outside `0.01..=100`).
const MIN_TIME_MULTIPLE: f32 = 0.01;
const MAX_TIME_MULTIPLE: f32 = 100.0;
/// The most particles one node makes in a frame, and all the nodes together: a datablock that asks for a particle a millisecond
/// of a clock a hundred times faster gets its first ones and loses the rest of that frame's time.
pub const MAX_SPAWNS_PER_NODE: usize = 64;
pub const MAX_SPAWNS_PER_FRAME: usize = 1024;
/// The longest stretch of one frame a node runs (ms).
const MAX_FRAME_MS: f32 = 1000.0;

/// One node's emitter and the clock it keeps.
#[derive(Debug, Clone)]
struct Node {
    /// The `ParticleEmitterData` the emitter was made of, and the datablock epoch it was made at.
    datablock: u32,
    epoch: u64,
    emitter: Option<Emitter>,
    /// The emitter's time owed (ms, under one after a frame).
    owed_ms: f32,
    /// Emitted this frame (a node that did not breaks its path).
    live: bool,
}

/// The emitters of the ghosted `ParticleEmitterNode`s (module docs).
#[derive(Debug, Clone)]
pub struct EmitterNodes {
    nodes: BTreeMap<u32, Node>,
    rng: Rng,
    live: usize,
    wind: [f32; 3],
    /// The real time owed to the emitters' particles (ms, under one after a frame).
    real_owed_ms: f32,
}

impl Default for EmitterNodes {
    fn default() -> EmitterNodes {
        EmitterNodes {
            nodes: BTreeMap::new(),
            rng: Rng::new(0x5eed_90de),
            live: 0,
            wind: [0.0; 3],
            real_owed_ms: 0.0,
        }
    }
}

/// `timeMultiple` of the `ParticleEmitterNodeData` `db` (1 when the client has no such datablock or it names no value).
fn time_multiple(world: &ClientWorld<TgeClientSet>, db: Option<u32>) -> f32 {
    let value = db
        .and_then(|id| world.datablocks.get(&id))
        .filter(|d| d.class == "ParticleEmitterNodeData")
        .and_then(|d| d.get("timeMultiple", 0));
    match value {
        Some(Value::F32(x)) if x.is_finite() => x.clamp(MIN_TIME_MULTIPLE, MAX_TIME_MULTIPLE),
        _ => 1.0,
    }
}

/// A node's place and its z axis, when its matrix and scale are finite.
fn placed(pose: &Pose) -> Option<([f32; 3], [f32; 3])> {
    let Pose::Matrix { m, scale } = pose else {
        return None;
    };
    if !m.iter().chain(scale).all(|c| c.is_finite()) {
        return None;
    }
    let m = Mat { m: *m };
    Some((m.column(3), m.mul_v([0.0, 0.0, 1.0])))
}

impl EmitterNodes {
    /// Forgets every node (a new connection's ghosts are new).
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.live = 0;
    }

    /// Forgets the emitter of ghost `index` and its particles (another object took its index).
    pub fn forget(&mut self, index: u32) {
        if let Some(node) = self.nodes.remove(&index)
            && let Some(e) = node.emitter
        {
            self.live = self.live.saturating_sub(e.particles.len());
        }
    }

    /// The air's velocity each emitter's particles are carried by.
    pub fn winds(&self) -> Vec<[f32; 3]> {
        self.nodes
            .values()
            .filter_map(|n| n.emitter.as_ref())
            .map(Emitter::wind)
            .collect()
    }

    /// The air's velocity the nodes' particles are carried by from now on.
    pub fn set_wind(&mut self, wind: [f32; 3]) {
        self.wind = wind;
        for node in self.nodes.values_mut() {
            if let Some(e) = node.emitter.as_mut() {
                e.set_wind(wind);
            }
        }
    }

    /// The particles alive.
    pub fn particles(&self) -> usize {
        self.nodes
            .values()
            .filter_map(|n| n.emitter.as_ref())
            .map(|e| e.particles.len())
            .sum()
    }

    /// The nodes with an emitter.
    pub fn emitters(&self) -> usize {
        self.nodes.values().filter(|n| n.emitter.is_some()).count()
    }

    /// A frame of `dt` seconds: every emitter's particles moved on, and the nodes the world ghosts emit. An emitter of a node
    /// gone lives on until its last particle died.
    pub fn update(&mut self, world: &ClientWorld<TgeClientSet>, dt: f32) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.real_owed_ms = (self.real_owed_ms + dt * 1000.0).min(MAX_FRAME_MS);
        let real_ms = self.real_owed_ms.floor();
        self.real_owed_ms -= real_ms;
        for node in self.nodes.values_mut() {
            node.live = false;
            if let Some(e) = node.emitter.as_mut() {
                e.advance(real_ms as u32, &mut self.live);
            }
        }
        let mut spawns_left = MAX_SPAWNS_PER_FRAME;
        for (&index, ghost) in &world.ghosts {
            let (Some(TgeClass::ParticleEmitterNode), Some(TgeRecord::ParticleEmitterNode(record))) =
                (ghost.class, ghost.record.as_ref())
            else {
                continue;
            };
            let Some(datablock) = record.emitter else {
                continue;
            };
            if !self.nodes.contains_key(&index) && self.nodes.len() >= MAX_NODES {
                continue;
            }
            let node = self.nodes.entry(index).or_insert_with(|| Node {
                datablock,
                epoch: u64::MAX,
                emitter: None,
                owed_ms: 0.0,
                live: false,
            });
            node.live = true;
            if node.epoch != world.datablock_epoch || node.datablock != datablock {
                if let Some(old) = node.emitter.take() {
                    self.live = self.live.saturating_sub(old.particles.len());
                }
                node.datablock = datablock;
                node.epoch = world.datablock_epoch;
                node.emitter = Emitter::new(world, datablock);
                if let Some(e) = node.emitter.as_mut() {
                    e.set_wind(self.wind);
                }
            }
            let Some(emitter) = node.emitter.as_mut() else {
                continue;
            };
            // The emission clock runs `timeMultiple` times the real one.
            node.owed_ms = (node.owed_ms + dt * time_multiple(world, ghost.datablock()) * 1000.0)
                .min(MAX_FRAME_MS);
            let whole = node.owed_ms.floor();
            node.owed_ms -= whole;
            let Some((at, axis)) = ghost.pose().as_ref().and_then(placed) else {
                emitter.break_path();
                continue;
            };
            // A call makes at most one particle more than its time over the shortest period (the period less its variance);
            // the time that would make more than the node's or the frame's share is not run.
            let shortest = (emitter.data.period_ms - emitter.data.period_variance_ms).max(1) as f32;
            let share = MAX_SPAWNS_PER_NODE.min(spawns_left);
            let ms = whole.min(share.saturating_sub(1) as f32 * shortest);
            let before = emitter.particles.len();
            let velocity = axis.map(|c| c * NODE_VELOCITY);
            emitter.emit_path(at, axis, velocity, ms as u32, &mut self.rng, &mut self.live);
            spawns_left =
                spawns_left.saturating_sub(emitter.particles.len().saturating_sub(before));
        }
        // A node not emitting this frame lets its path go; a gone node's emitter runs its particles out, then goes.
        let mut gone = 0;
        self.nodes.retain(|_, node| {
            let Some(emitter) = node.emitter.as_mut() else {
                return node.live;
            };
            if !node.live {
                emitter.break_path();
            }
            let keep = node.live || !emitter.particles.is_empty();
            if !keep {
                gone += emitter.particles.len();
            }
            keep
        });
        self.live = self.live.saturating_sub(gone);
    }

    /// The particles made since the last hand-over into `out`, for the GPU particle pool ([`Emitter::hand_over`]) instead of
    /// [`EmitterNodes::draws`].
    pub fn hand_over(&mut self, out: &mut SpriteFrame, textures: &mut TextureSet<'_>) {
        for node in self.nodes.values_mut() {
            if let Some(e) = node.emitter.as_mut() {
                e.hand_over(out, textures, "node", &mut self.live);
            }
        }
    }

    /// The particles as draws, each emitter at its center's distance from `eye`, billboards along the camera's `right` and `up`.
    pub fn draws(
        &self,
        eye: [f32; 3],
        right: [f32; 3],
        up: [f32; 3],
        textures: &mut TextureSet<'_>,
    ) -> Vec<(f32, Draw)> {
        self.nodes
            .values()
            .filter_map(|n| {
                let e = n.emitter.as_ref()?;
                let c = e.centre()?;
                let d = e.draw(right, up, textures, "node")?;
                let dist = (0..3).map(|i| (c[i] - eye[i]).powi(2)).sum::<f32>().sqrt();
                Some((dist, d))
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "emitter_nodes_tests.rs"]
mod tests;
