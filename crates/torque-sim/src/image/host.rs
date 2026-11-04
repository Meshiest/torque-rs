//! What the image state machine needs from the host: the slot store and the datablocks, the owner's energy and network mask, and the
//! script callbacks at their call points. Callbacks are synchronous: the host runs the script inside the call, and a script that calls
//! `mountImage`, `unmountImage`, `setImageTrigger`, `setImageAmmo` or `setImageLoaded` reaches the functions of [`super::machine`] with
//! the same host (see the callback contract in the module docs).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/shapeImage.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

use std::sync::Arc;

use torque_collide::TypeMasks;
use torque_collide::math::{F3, Matrix};

use super::{DbId, ImageData, ImageSet};
use crate::RayHit;
use crate::player::PlayerState;

/// The controlling client's two fields `getMuzzleVector` reads: its camera position and the AI-client flag.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AimClient {
    /// The connection's camera position; the aim is corrected only in the first person (exactly 0.0).
    pub freelook_value: f32,
    /// An AI client; no corrected aim then.
    pub ai: bool,
}

/// `Container::castRay` for the muzzle chain.
pub trait RayCaster {
    /// The nearest hit along `start` to `end` with `mask`, this object excluded for the duration (the collision-disable
    /// counter of the object and of everything mounted on it around the casts); `exclude_mount` also excludes the object
    /// this one is mounted on (the corrected aim does that).
    fn cast_ray(&mut self, start: F3, end: F3, mask: u32, exclude_mount: bool) -> Option<RayHit>;
    /// The masks of the aim ray: Torque's unless the host claims bits of its own.
    fn type_masks(&self) -> &TypeMasks {
        &TypeMasks::TGE
    }
    /// `getMuzzlePoint`'s point for the owner `s`: Torque's is `muzzle`, the translation of the muzzle transform (Player's
    /// override of it when `player`); `mount` is the translation of the slot's mount transform, made first. A host may move
    /// the point (with [`RayCaster::cast_ray`]); the default keeps Torque's.
    fn muzzle_point(&mut self, s: &PlayerState, mount: F3, muzzle: F3, player: bool) -> F3 {
        let _ = (s, mount, player);
        muzzle
    }
    /// The corrected aim: from the muzzle toward what lies straight ahead of the eye, or `None` to keep the muzzle's own
    /// direction. `eye` is the eye transform, `muzzle` the translation of the muzzle transform and `point` what
    /// [`RayCaster::muzzle_point`] made of it. The default ([`super::muzzle::corrected_aim`]) aims from `muzzle`.
    fn corrected_aim(&mut self, eye: &Matrix, muzzle: F3, point: F3) -> Option<F3> {
        let _ = point;
        super::muzzle::corrected_aim(self, eye, muzzle)
    }
}

/// The host's side of the image step; one host is bound to one object.
pub trait ImageHost {
    /// The object's four slots. Called at every access: nothing holds the reference across a callback.
    fn images(&mut self) -> &mut ImageSet;
    /// A `ShapeBaseImageData` by id (`None`: it is gone; the call then does nothing).
    fn image_data(&mut self, db: DbId) -> Option<Arc<ImageData>>;

    /// `setMaskBits(bits)` on the object (`0x100000 << slot` for the image blocks).
    fn raise_mask(&mut self, bits: u32);
    /// `getEnergyLevel`: the object's energy, or the mount chain's while the datablock's `inheritEnergyFromMount` is set
    /// (0 at the end of the chain).
    fn energy(&mut self) -> f32;
    /// `setEnergyLevel`.
    fn set_energy(&mut self, energy: f32);

    /// `Con::executef(db, 3, script, %obj, %slot)` (with the state's script name): the image's `stateScript`. `db` is the slot's
    /// datablock at the call (the live field).
    fn state_script(&mut self, db: DbId, script: &str, slot: usize);
    /// `%db.onMount(%obj, %slot)`, after the first state's entry; `db` is the slot's datablock at the call.
    fn on_mount(&mut self, db: DbId, slot: usize);
    /// `%db.onUnmount(%obj, %slot)`, before the teardown.
    fn on_unmount(&mut self, db: DbId, slot: usize);
    /// `on_recoil(slot, recoil)`: `Player` restarts its arm thread; `ShapeBase`'s is empty. The thread moves the arm node,
    /// which the muzzle chain reads, so the host's skeleton must see it.
    fn on_recoil(&mut self, _slot: usize, _recoil: i32) {}
    /// `TSShapeInstance::animate` on the owner's own shape instance: `setImageState`
    /// runs it at the top of each pass whose state is the datablock's `fireState` (server, and ghostable objects), before
    /// the same-state test. The nodes are animated with the threads as they are; nothing else changes.
    fn fire_animate(&mut self) {}
    /// The light registry (the add call on a mount of an image with a `lightType`, the remove call when the last such image is
    /// unmounted); no state the simulation reads.
    fn light_registry(&mut self, _add: bool) {}

    /// `Sim::findObject(name-or-id)` cast to `ShapeBaseImageData` (`mountImage`'s first argument).
    fn find_image_data(&mut self, _name: &str) -> Option<DbId> {
        None
    }
    /// True when the call comes from a client-side connection test: the current `GameConnection` exists and is not a
    /// server-side connection (`mountImage` and `getMuzzlePoint` do nothing then).
    fn client_side_call(&mut self) -> bool {
        false
    }
}
