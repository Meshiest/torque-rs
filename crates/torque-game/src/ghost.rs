//! The game classes' link to the network layer that ghosts them ([`GhostLink`]): an object registered for ghosting keeps the
//! link its host gave it, and the classes' hooks mark its state dirty, ask whether it is ghosted and ask about the connection
//! that controls it through it. A host without a network layer gives no link: every call answers as for an object no connection
//! knows.

use std::any::Any;

use ts_vm::sim::ObjId;
use ts_vm::vm::Vm;

/// What an object's classes ask of the layer that ghosts it.
pub trait GhostLink {
    /// `setMaskBits`: marks `bits` of the object dirty for every connection that ghosts it.
    fn set_mask(&self, obj: ObjId, bits: u32);
    /// `clearMaskBits`.
    fn clear_mask(&self, obj: ObjId, bits: u32);
    /// Whether the object is registered for ghosting.
    fn ghosted(&self, obj: ObjId) -> bool;
    /// The mask the class's own `setTransform` marks (by the object's ghost class), 0 for a class whose `setTransform` marks
    /// nothing.
    fn transform_mask(&self, obj: ObjId) -> u32;
    /// `Container::checkBins` for the object, which the layer can reach (after a `setTransform` the host makes itself).
    fn check_bins(&self, vm: &Vm, obj: ObjId);
    /// `ShapeBase::getControllingClient`: the connection object whose control chain holds the object.
    fn controlling_client(&self, vm: &Vm, obj: ObjId) -> Option<ObjId>;
    /// The camera position of the controlling client.
    fn controlling_camera_pos(&self, vm: &Vm, obj: ObjId) -> Option<f32>;
    /// Whether some connection controls the object directly.
    fn is_control_object(&self, obj: ObjId) -> bool;
    /// The connection whose control chain holds the object loses its control object.
    fn release_controlling_connection(&self, vm: &Vm, obj: ObjId);
    /// The layer itself, for its own code.
    fn as_any(&self) -> &dyn Any;
    /// The record bits marked on the object that no connection has taken yet (`mDirtyMaskBits`, in the record's numbering;
    /// 0 for a layer that keeps none).
    fn pending_mask(&self, obj: ObjId) -> u32 {
        let _ = obj;
        0
    }
    /// Takes `bits` (record numbering) back off the object's pending bits before any connection took them.
    fn drop_pending(&self, obj: ObjId, bits: u32) {
        let _ = (obj, bits);
    }
    /// Whether every connection that ghosts holds a ghost of the object (in scope, not going): what a client draws on it
    /// can be drawn on every client. By default, whether the object is registered for ghosting.
    fn ghosted_everywhere(&self, obj: ObjId) -> bool {
        self.ghosted(obj)
    }
    /// `setMaskBits` in the record's own numbering, for bits no class simulation marks (the per-ghost block's).
    fn set_record_mask(&self, obj: ObjId, bits: u32) {
        let _ = (obj, bits);
    }
}

impl std::fmt::Debug for dyn GhostLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GhostLink")
    }
}
