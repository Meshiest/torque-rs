//! What the client's engine half asks the renderer: [`PoseView`] poses a ghost's shape as the frame draws it, for the eye
//! ([`crate::eye`]: the `eye` node, the mount node of a rider) and the HUD's labels ([`crate::hud`]); [`RenderResets`] is told
//! when what the renderer looked up goes stale (a new connection, the server's files). The game's view of the client's world
//! implements [`PoseView`] and its renderer [`RenderResets`].

use std::ops::Deref;

use ghost_world::{ClientSet, ClientWorld};
use shapes::instance::ShapeInstance;
use shapes::shape::Shape;

/// A ghost's record in a view's set.
pub type RecordOf<V> = <<V as PoseView>::Set as ClientSet>::Record;

/// What a Player's pose reads of its record besides the head pitch and the crouch: the sequences of its arm and action threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PoseInputs {
    pub arm: Option<u32>,
    pub action: Option<u32>,
}

/// The client's world as the renderer poses its ghosts: the record world (through `Deref`) and the renderer's posing over it.
pub trait PoseView: Deref<Target = ClientWorld<Self::Set>> {
    type Set: ClientSet;

    /// What the pose of `record` reads, `None` for a record the renderer does not pose as a Player (its shape stands at its
    /// default pose).
    fn pose_inputs(&self, record: &RecordOf<Self>) -> Option<PoseInputs>;

    /// Whether `record`'s last move left it crouched.
    fn record_crouched(&self, record: &RecordOf<Self>) -> bool;

    /// Whether ghost `index` is crouched now (the client's own prediction for its control object, else its record's).
    fn crouched(&self, index: u32) -> bool;

    /// Adds the sequences a shape constructor names (its `sequence<N>` values) to `shape`, whose file is in folder `dir`.
    fn import_sequences(&self, shape: &mut Shape, dir: &str, sequences: &[Vec<u8>]);

    /// Poses `instance` of `shape` as the renderer poses `record` (one [`PoseView::pose_inputs`] answers for) at head pitch
    /// `pitch`, crouched or not.
    fn pose(
        &self,
        shape: &Shape,
        instance: &mut ShapeInstance,
        record: &RecordOf<Self>,
        pitch: f32,
        crouched: bool,
    );

    /// Ghost `index`'s shape (of datablock `db`) at the pose the frame draws it in (its threads at the frame's clock), `None`
    /// without one.
    fn pose_live(
        &self,
        index: u32,
        db: u32,
        shape: &Shape,
        record: &RecordOf<Self>,
        pitch: f32,
    ) -> Option<ShapeInstance>;

    /// The render transform of ghost `index` riding a mount (the mount's `mount<N>` node as the frame draws it), `None` when it
    /// rides none.
    fn mount_render(&self, index: u32) -> Option<[f32; 16]>;
}

/// What the renderer is told when what it looked up goes stale. None of it needs the VM: a connection's end runs them.
pub trait RenderResets {
    /// A new connection: the last one's live poses are forgotten.
    fn connection_reset(&self) {}

    /// A file arrived: the names that found nothing are looked up again.
    fn files_added(&self) {}

    /// The server's files changed or went: every name found before is looked up again.
    fn files_replaced(&self) {}
}

/// No renderer to tell.
#[derive(Debug, Default)]
pub struct NoRenderer;

impl RenderResets for NoRenderer {}
