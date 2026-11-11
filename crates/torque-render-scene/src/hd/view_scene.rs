//! The scene a world view fills ([`ViewScene`]).

use std::ops::DerefMut;

use super::HdMode;
use super::cells::StaticCells;

/// The scene a world view fills: the engine's frame by deref, and the parts the view sets beside it. An extension whose scene
/// is one can be drawn by a world view (`torque_app::view::WorldView<E>`); its own data stays in the scene beside the frame,
/// filled by the game.
pub trait ViewScene: Default + DerefMut + 'static {
    /// How the scene is drawn from the next frame (None: not by the enhanced renderer).
    fn set_mode(&mut self, mode: Option<HdMode>);

    /// The effects' time (seconds).
    fn set_time(&mut self, seconds: f32);

    /// The cells the view's static meshes and batches go in (None: this scene's geometry takes none, and the view refuses
    /// them).
    fn batch_cells(&mut self) -> Option<&mut StaticCells> {
        None
    }
}

#[cfg(test)]
#[path = "view_scene_tests.rs"]
mod tests;
