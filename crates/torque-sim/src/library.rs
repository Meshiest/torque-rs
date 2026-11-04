//! The simulation library table: the steps a host may install other implementations of, called through one set of function
//! pointers as `torque_collide::library` does for the vector and matrix routines.
//!
//! Every entry of [`SimLibrary`] is called through [`get`]. [`PLAIN`] holds this crate's own code, plain `f32` arithmetic in
//! the order Torque's source writes it; a host installs another table once, before its first step, with [`install`].

use std::sync::OnceLock;

use torque_collide::Exhausted;
use torque_collide::math::{F3, Matrix};

use crate::Move;
use crate::item::{Item, ItemData, ItemHost, ItemState};
use crate::player::{Player, PlayerData, PlayerHost, PlayerState};

/// The steps called through the table. Each entry names the Torque routine it stands for.
#[derive(Debug, Clone, Copy)]
pub struct SimLibrary {
    /// `Player::updateMove`.
    pub player_update_move:
        fn(&mut Player, &PlayerData, &Move, &mut dyn PlayerHost) -> Result<(), Exhausted>,
    /// `Player::updatePos(dt)`.
    pub player_update_pos:
        fn(&mut Player, &PlayerData, f32, &mut dyn PlayerHost) -> Result<(), Exhausted>,
    /// `Player::updateLookAnimation`.
    pub player_look_animation: fn(&mut PlayerState, &PlayerData, &mut dyn PlayerHost),
    /// `PlayerData::preload`'s products, with the side the pickup radius is measured from.
    pub player_preload: fn(&mut PlayerData, f32),
    /// `MatrixF::affineInverse` as `setTransform` keeps the world-to-object matrix.
    pub affine_inverse: fn(&Matrix) -> Matrix,
    /// `Item::processTick`.
    pub item_process_tick: fn(&mut Item, &ItemData, &mut dyn ItemHost) -> Result<(), Exhausted>,
    /// `Item::setVelocity`'s velocity (the mask and the wake-up are the caller's).
    pub item_set_velocity: fn(&mut ItemState, F3),
    /// `Item::setTransform`'s yaw of a matrix (`atan2(-m[1], m[5])`).
    pub item_yaw: fn(&Matrix) -> f32,
    /// `ShapeBase::getCorrectedAim` after its ray: the unit vector from the muzzle (third) toward the point the ray met
    /// (second), pulled out from the ray's start (first) as the module describes; `None` when the muzzle is at that point.
    pub aim_vector: fn(F3, F3, F3) -> Option<F3>,
    /// The muzzle vector's normalisation (`getMuzzleVector`'s console method).
    pub muzzle_normalize: fn(F3) -> F3,
    /// `ShapeBase::updateImageState`'s energy step: `energy - drain * dt`.
    pub image_energy: fn(f32, f32, f32) -> f32,
}

/// This crate's own steps: plain `f32` arithmetic in the order Torque's source writes it.
pub static PLAIN: SimLibrary = SimLibrary {
    player_update_move: Player::plain_update_move,
    player_update_pos: Player::plain_update_pos,
    player_look_animation: crate::player::look::plain_update_look_animation,
    player_preload: PlayerData::plain_preload_with,
    affine_inverse: crate::player::transform::plain_affine_inverse,
    item_process_tick: Item::plain_process_tick,
    item_set_velocity: crate::item::plain_set_velocity,
    item_yaw: crate::item::plain_yaw,
    aim_vector: crate::image::muzzle::plain_aim_vector,
    muzzle_normalize: crate::image::muzzle::plain_console_normalize,
    image_energy: crate::image::machine::plain_energy,
};

static INSTALLED: OnceLock<&'static SimLibrary> = OnceLock::new();

/// The table in use: the installed one, else [`PLAIN`].
#[inline]
pub fn get() -> &'static SimLibrary {
    match INSTALLED.get() {
        Some(lib) => lib,
        None => &PLAIN,
    }
}

/// Installs `library` for the rest of the process. Only the first install takes; installing the same table again is a
/// no-op. Returns false when another table was installed first.
pub fn install(library: &'static SimLibrary) -> bool {
    std::ptr::eq(*INSTALLED.get_or_init(|| library), library)
}
