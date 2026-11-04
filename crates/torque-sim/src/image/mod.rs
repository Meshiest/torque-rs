//! The weapon image state machine: `ShapeBase`'s four `MountedImage` slots, `updateImageState`, `setImageState`, `setImage`,
//! `setImageTrigger`, `mountImage`/`unmountImage`, the 18 console methods that read or write them, and the muzzle point and vector.
//!
//! **This module is the server's images** (`netFlags` bit 1, the ghost bit, clear). The ghost-only work (shape threads, sounds, emitters,
//! eject shells, texture loads, the spin scale, the random flash draw) draws no random number and sets no state a server reads, so it is
//! not here. Entering the fire state animates the owner's shape instance (`TSShapeInstance::animate` at its current detail,
//! on a server for a Ghostable object): the node matrices the muzzle and slot transforms read change, so the host is told
//! ([`ImageHost::fire_animate`]).
//!
//! **Arithmetic:** plain `f32`; the energy step and the muzzle vector's aim and normalisation go through the
//! [`library`](crate::library) table. A comparison is written so that a NaN takes a fixed side.
//!
//! **The callback contract.** Script runs in the middle of `setImageState` (the state's `stateScript`) and of `setImage`
//! (`onUnmount` before the teardown, `onMount` after the first state's entry): the script may call `mountImage`, `unmountImage`,
//! `setImageTrigger`, `setImageAmmo` and `setImageLoaded` on the same object, even on the slot being changed. So:
//! - the slot store ([`ImageSet`]) is owned by the host and reached through [`ImageHost::images`] at every access: no borrow of it lives
//!   across a callback, and a nested call (the host's native calls [`machine::set_image_trigger`] and the rest with the same host) sees
//!   the live fields;
//! - the datablock is held through an `Arc<ImageData>` taken at the top of each loop pass, so the cached state record whose timeout is
//!   read after the script is the one the pass started with, as a pointer into the datablock would be;
//! - the fields that are re-read after the script (`delay`, `db`) are re-read from the store.
//!
//! **Caps:** without them a hostile datablock (a zero-timeout cycle of states) or a hostile script (an `onUnmount` that mounts
//! another image into the same slot) would loop or recurse without end:
//! [`MAX_IMAGE_DEPTH`] bounds the nesting of `setImageState`/`setImage`/`updateImageState` through scripts, [`MAX_IMAGE_CHAIN`] bounds
//! the state transitions of one outermost call, and [`MAX_STATES`] bounds the states of a datablock (31, the size of the state
//! arrays). A cap trip ends the call where it stands and counts in [`ImageSet::cap_hits`].
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/shapeImage.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

// Comparisons are written so that a NaN takes a fixed side: a negated comparison is the intended form.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod console;
pub mod data;
pub mod driver;
pub mod host;
pub mod machine;
pub mod muzzle;

#[cfg(test)]
mod tests;

pub use data::{ImageData, ImageState, RawState};
pub use host::{AimClient, ImageHost, RayCaster};
pub use machine::{
    mount_image, set_image, set_image_ammo, set_image_loaded, set_image_state, set_image_trigger,
    tick_images, unmount_image, update_image_state,
};

/// The mounted-image slots of a `ShapeBase`.
pub const SLOTS: usize = 4;
/// States per `ShapeBaseImageData` (the arrays of 31).
pub const MAX_STATES: usize = 31;
/// The datablock id (`SimObject` id) of a `ShapeBaseImageData`.
pub type DbId = u32;

/// The step the object's tick gives every slot (`TickSec`), in seconds.
pub const IMAGE_DT: f32 = 0.032;
/// `setMaskBits(0x100000 << slot)`: the network mask bit of slot `slot`'s image block (bits 20..23).
pub const fn slot_mask(slot: usize) -> u32 {
    0x10_0000u32 << (slot as u32 & 31)
}

/// The nesting limit of `setImageState`, `setImage` and `updateImageState` calls through script callbacks.
pub const MAX_IMAGE_DEPTH: u32 = 16;
/// The state transitions one outermost call may make (a zero-timeout cycle would never end; the 31 states of a sane
/// datablock chain in at most 31).
pub const MAX_IMAGE_CHAIN: u32 = 256;

/// One mounted image (the ghost-only fields, the thread and emitter pointers, are not here).
#[derive(Debug, Clone, PartialEq)]
pub struct MountedImage {
    /// The datablock (`None`: the slot is empty).
    pub db: Option<DbId>,
    /// The current state, an index into the datablock's states.
    pub state: usize,
    /// The image to switch to when a state with `allowImageChange` is entered (1 = none; 0 = a pending unmount, which
    /// nothing on the server stores).
    pub pending: Option<DbId>,
    /// The skin (a net-string handle, 0 none).
    pub skin: u32,
    /// The pending image's skin, and its `loaded` argument.
    pub pending_skin: u32,
    pub pending_loaded: bool,
    /// Loaded.
    pub loaded: bool,
    /// The time left in the state, in seconds.
    pub delay: f32,
    /// The fire counter (`+1 & 7`, the three bits on the wire).
    pub fire_count: u32,
    /// The trigger.
    pub trigger_down: bool,
    /// Ammo.
    pub ammo: bool,
    /// Target.
    pub target: bool,
    /// Wet.
    pub wet: bool,
}

impl MountedImage {
    /// An empty slot (the object's constructor zeroes the block and sets `pending` to 1).
    pub const fn empty() -> MountedImage {
        MountedImage {
            db: None,
            state: 0,
            pending: None,
            skin: 0,
            pending_skin: 0,
            pending_loaded: false,
            loaded: false,
            delay: 0.0,
            fire_count: 0,
            trigger_down: false,
            ammo: false,
            target: false,
            wet: false,
        }
    }
}

/// The four slots, plus this crate's guards. The host owns one per object and answers [`ImageHost::images`] with it.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageSet {
    pub slots: [MountedImage; SLOTS],
    /// Nesting depth of the calls that run script (see [`MAX_IMAGE_DEPTH`]).
    pub(crate) depth: u32,
    /// Transitions made by the current outermost call (see [`MAX_IMAGE_CHAIN`]).
    pub(crate) steps: u32,
    /// How many times a cap ended a call.
    pub cap_hits: u32,
}

impl Default for ImageSet {
    fn default() -> Self {
        ImageSet::new()
    }
}

impl ImageSet {
    pub const fn new() -> ImageSet {
        ImageSet {
            slots: [
                MountedImage::empty(),
                MountedImage::empty(),
                MountedImage::empty(),
                MountedImage::empty(),
            ],
            depth: 0,
            steps: 0,
            cap_hits: 0,
        }
    }

    /// Which slots hold an image: what `PlayerState::images` mirrors.
    pub fn occupied(&self) -> [bool; SLOTS] {
        [
            self.slots[0].db.is_some(),
            self.slots[1].db.is_some(),
            self.slots[2].db.is_some(),
            self.slots[3].db.is_some(),
        ]
    }

    /// Copies the occupancy into the Player's mirror (`PlayerState::images`).
    pub fn mirror_into(&self, images: &mut [bool; SLOTS]) {
        *images = self.occupied();
    }

    /// Takes the Player's slot-0 wet flag (`PlayerState::image_wet`, which the tick flips before the image loop) into
    /// the slot. Torque keeps one flag; the Player step keeps the mirror, and the host moves it here before each tick.
    pub fn load_wet0(&mut self, wet: bool) {
        self.slots[0].wet = wet;
    }
}
