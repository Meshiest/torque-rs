//! The one-shot things a client does that the frame and the speakers show: the sounds the server asks for (`Sim2DAudioEvent`,
//! `Sim3DAudioEvent`: `alxPlay` of the profile, at the event's position for the 3D one), the explosions a projectile's ghost carries
//! (`Projectile::unpackUpdate`'s explosion block: `explode` makes an `Explosion` of the datablock's `explosion` on the client), and
//! what the mounted images' states ask for ([`super::images::ImageEvent`]: state sounds, state emitters).
//!
//! The world keeps them in order in a bounded queue ([`MAX_EFFECTS`]; the oldest are dropped and counted), each with a sequence number
//! and the clock it came at; each consumer (a client host's audio, a renderer's particle effects) keeps the last number it took.
//!
//! Credit: the events and calls named here are TGE's, as Torque3D (MIT) carries them in `T3D/gameBase/gameConnectionEvents.cpp`,
//! `T3D/projectile.cpp` and `T3D/shapeBase.cpp` (each Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.).

use super::images::ImageEvent;

/// Engine cap: the effects the world keeps for its consumers.
pub const MAX_EFFECTS: usize = 4096;

/// One effect; `G` is the game's own ([`super::set::ClientSet::Effect`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Effect<G> {
    /// A mounted image's state asked for something.
    Image {
        ghost: u32,
        slot: usize,
        event: ImageEvent,
    },
    /// `Sim2DAudioEvent`: the profile played where the listener is.
    Sound2D { profile: u32 },
    /// `Sim3DAudioEvent`: the profile played at a point, turned by the cone's quaternion when its description has a cone.
    Sound3D {
        profile: u32,
        position: [f32; 3],
        cone: Option<[f32; 4]>,
    },
    /// A projectile ghost exploded: the `ExplosionData` its datablock names, at the point the server sent, facing its normal.
    Explosion {
        ghost: u32,
        datablock: u32,
        position: [f32; 3],
        normal: [f32; 3],
    },
    /// A ghost left the client (its sounds and emitters end with it).
    GhostGone { ghost: u32 },
    /// A ShapeBase sound slot (`playAudio`, `stopAudio`) as an update carried it (`ShapeBase::unpackUpdate`'s sound block, then
    /// `updateAudioState`): the slot's sound stops, and with a profile one plays at the object (following it).
    ShapeSound {
        ghost: u32,
        slot: usize,
        profile: Option<u32>,
    },
    /// The game's own effect.
    Game(G),
}

/// An effect with its number and the clock it came at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectEvent<G> {
    pub seq: u64,
    pub ms: Option<u32>,
    pub effect: Effect<G>,
}
