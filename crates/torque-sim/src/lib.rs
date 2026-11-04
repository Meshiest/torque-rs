//! The simulation a client predicts: the process list, Player and AIPlayer, Item, Projectile and the weapon images, as pure state
//! and steps over `collide`. Std only: scripting, callbacks and the network belong to the host that embeds this crate.
//!
//! - [`process`]: the process list and the seam the objects tick through ([`Ticking`], [`Move`]).
//! - [`player`]: the Player's server step, `processTick` through `updateMove` and `updatePos`, the packet-data words, the host and
//!   skeleton seams, the subclass seam ([`player::PlayerClass`], [`player::PlayerFields`]), and the driver that runs a scripted
//!   move list.
//! - [`ai`]: the AIPlayer: `getAIMove`, the `updateMove` override and the console methods' state effects.
//! - [`item`]: the Item's `processTick` with `updateVelocity`, `updateWorkingCollisionSet` and `updatePos`, and its script entry
//!   points.
//! - [`projectile`]: `processTick`, `explode` and the datablock validators.
//! - [`image`]: the weapon images: `updateImageState`, `setImageState`, `setImage`, the mount and trigger effects, the console
//!   methods and the muzzle point and vector.
//! - [`library`]: the table of steps whose arithmetic a host may replace ([`library::SimLibrary`]); this crate's own is plain
//!   `f32` arithmetic in the order Torque's source writes it.
//!
//! The process list, the Player, the AIPlayer, the Item, the Projectile and the images follow TGE 1.4's design; each module
//! names the Torque3D file it follows where Torque3D has the same rules, with that file's copyright lines.

pub mod ai;
pub mod image;
pub mod item;
pub mod library;
pub mod player;
pub mod process;
pub mod projectile;
mod scene;
#[cfg(test)]
mod testworld;

pub use scene::{DT, MAX_COLLISION_QUEUE, QueuedCollision, RayHit, SceneObject, queue_collision};

/// An object's key in the process list: the id the scripts see.
pub type ObjectKey = u32;

/// The triggers a move carries (`MaxTriggerKeys`).
pub const MAX_TRIGGERS: usize = 6;

/// One move as `processTick` reads it: the floats the wire's raw values were turned into, the
/// free-look flag and the triggers. This crate has no dependency on a protocol crate, so the host converts.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Move {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub free_look: bool,
    pub triggers: [bool; MAX_TRIGGERS],
}

/// An object the process list ticks (`GameBase::processTick`). `Cx` is what a tick needs from the host (the script
/// VM, the budget of the script calls): this crate stays std only, the host's adapters implement this over their own context type.
pub trait Ticking<Cx: ?Sized> {
    /// The class's name, for diagnostics and for messages.
    fn class(&self) -> &'static str;

    /// The object this one must be ticked after (`GameBase::processAfter`) when it enters the list, if its class asks for
    /// one at its `onAdd` (a Projectile follows the object that fired it). Later changes go through
    /// [`ProcessList::process_after`](process::ProcessList::process_after).
    fn process_after(&self) -> Option<ObjectKey> {
        None
    }

    /// One tick: `Some(move)` for a connection's control object, once per move; `None` for every other object.
    fn process_tick(&mut self, cx: &mut Cx, mv: Option<&Move>);
}
