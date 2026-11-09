//! The scene classes' hooks into the layer that ghosts an object (the object's [`GhostLink`], kept in its ext): each answers as
//! for an object no connection knows when the object has no link.

use std::rc::Rc;

use crate::ghost::GhostLink;
use ts_vm::sim::ObjId;
use ts_vm::vm::Vm;

use super::ext;

fn link(vm: &Vm, obj: ObjId) -> Option<Rc<dyn GhostLink>> {
    ext::get(vm.objects(), obj)?.ghost.clone()
}

/// `setMaskBits`: marks `bits` of the object dirty for every connection that ghosts it (a pointer test for an object no layer
/// knows).
pub fn mark(vm: &Vm, obj: ObjId, bits: u32) {
    if let Some(link) = link(vm, obj) {
        link.set_mask(obj, bits);
    }
}

/// `clearMaskBits`.
pub fn unmark(vm: &Vm, obj: ObjId, bits: u32) {
    if let Some(link) = link(vm, obj) {
        link.clear_mask(obj, bits);
    }
}

/// The record bits marked on the object that no connection has taken yet (0 for an object no layer knows).
pub fn pending_mask(vm: &Vm, obj: ObjId) -> u32 {
    link(vm, obj).map_or(0, |l| l.pending_mask(obj))
}

/// Takes record bits back off the object's pending bits ([`GhostLink::drop_pending`]).
pub fn drop_pending(vm: &Vm, obj: ObjId, bits: u32) {
    if bits != 0
        && let Some(link) = link(vm, obj)
    {
        link.drop_pending(obj, bits);
    }
}

/// `setMaskBits` in the record's own numbering ([`GhostLink::set_record_mask`]).
pub fn mark_record(vm: &Vm, obj: ObjId, bits: u32) {
    if bits != 0
        && let Some(link) = link(vm, obj)
    {
        link.set_record_mask(obj, bits);
    }
}

/// Whether every connection that ghosts holds a ghost of the object ([`GhostLink::ghosted_everywhere`]).
pub fn ghosted_everywhere(vm: &Vm, obj: ObjId) -> bool {
    link(vm, obj).is_some_and(|l| l.ghosted_everywhere(obj))
}

/// Whether the object is registered for ghosting.
pub fn is_ghosted(vm: &Vm, obj: ObjId) -> bool {
    link(vm, obj).is_some_and(|l| l.ghosted(obj))
}

/// The mask the class's own `setTransform` marks, 0 for none (or no link).
pub fn transform_mask(vm: &Vm, obj: ObjId) -> u32 {
    link(vm, obj).map_or(0, |l| l.transform_mask(obj))
}

/// `Container::checkBins` for an object the layer knows.
pub fn check_bins(vm: &Vm, obj: ObjId) {
    if let Some(link) = link(vm, obj) {
        link.check_bins(vm, obj);
    }
}

/// `ShapeBase::getControllingClient`.
pub fn controlling_client(vm: &Vm, obj: ObjId) -> Option<ObjId> {
    link(vm, obj)?.controlling_client(vm, obj)
}

/// The camera position of the controlling client.
pub fn controlling_camera_pos(vm: &Vm, obj: ObjId) -> Option<f32> {
    link(vm, obj)?.controlling_camera_pos(vm, obj)
}

/// Whether some connection controls the object directly.
pub fn is_control_object(vm: &Vm, obj: ObjId) -> bool {
    link(vm, obj).is_some_and(|l| l.is_control_object(obj))
}

/// The connection whose control chain holds the object loses its control object.
pub fn release_controlling_connection(vm: &Vm, obj: ObjId) {
    if let Some(link) = link(vm, obj) {
        link.release_controlling_connection(vm, obj);
    }
}
