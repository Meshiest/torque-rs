//! The seam the per-ghost block is written through ([`torque_ghosts::block`]): a plugin that fills a section installs a
//! [`GhostBlocks`] in the server's script engine, and every record [`super::torque::TgeSource`] packs for a peer of the
//! engine's own is followed by the block. The same source scopes an object's relatives with it ([`GhostBlocks::relatives`]),
//! orders their first records parents first ([`GhostBlocks::height`]) and stamps its full query with what the block's links
//! change ([`GhostBlocks::revision`]). The same seam says which frame a control object's packet data is in
//! ([`GhostBlocks::control_frame`]: a controlled child's parent's, once the connection has the parent's ghost). A server
//! without one sends no block and no frame, as Torque's does.

use std::rc::Rc;

use torque_ghosts::block::BlockSections;
use torque_net::bits::BitWriter;
use torque_net::ghost::ConnectionGhosts;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

/// What fills the per-ghost block of a server's records (module docs). Each method takes `&self` and may be called while a
/// pack holds the server's script engine: it must not run a script or write a transform.
pub trait GhostBlocks {
    /// The sections the block carries; their plugins are in the manifest, so every peer of the engine's own reads them.
    fn sections(&self) -> BlockSections;

    /// Writes `obj`'s block for the dirty block bits `mask` (within [`torque_ghosts::block::BLOCK_MASK`]) for the
    /// connection whose ghosts are `ghosts`, and returns the record bits to send again: the block's own while a link waits
    /// for a parent the connection holds but has not acknowledged, and the class record's pose bits when the record must
    /// carry the object's pose (its parent will never reach that client). It must be repeatable, as a class record's pack
    /// is, and must not reach the ghost layer (it holds the layer while it packs).
    fn write(
        &self,
        obj: ObjId,
        mask: u32,
        ghosts: &dyn ConnectionGhosts,
        w: &mut BitWriter,
        ghost_bits: u8,
    ) -> u32;

    /// The objects scoped with `obj` when it is first scoped (its parent and its children, appended to `out`): following
    /// them in turn scopes a whole tree.
    fn relatives(&self, obj: ObjId, out: &mut Vec<ObjId>);

    /// The links below `obj` to its deepest descendant (0 for an object with no children).
    fn height(&self, obj: ObjId) -> u32;

    /// The block bits of other objects due on a connection once `obj` gets its ghost there (appended to `out`): its
    /// children's links, which name it by that ghost.
    fn marks_on_ghost(&self, obj: ObjId, out: &mut Vec<(ObjId, u32)>);

    /// A number that moves whenever a link is made or taken off.
    fn revision(&self) -> u64;

    /// The frame the packet data of control object `obj` is in for a connection whose acknowledged ghosts `ghost_index`
    /// names: its parent's, when it has a parent whose first record that connection acknowledged (`None`: the world's).
    /// Called as the head is written, with the server's script engine (the parent's nodes); the default has none.
    fn control_frame(
        &self,
        vm: &Vm,
        obj: ObjId,
        ghost_index: &dyn Fn(ObjId) -> Option<u32>,
    ) -> Option<ControlFrame> {
        let _ = (vm, obj, ghost_index);
        None
    }
}

/// A control object's frame ([`GhostBlocks::control_frame`]): where it hangs and its local pose there, rounded to the wire's
/// steps as its clients have it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlFrame {
    /// The parent's ghost index on the connection.
    pub ghost: u32,
    /// The node of the parent's shape it hangs on (`None`: the origin).
    pub node: Option<u16>,
    /// The parent's world times the node: the frame's own transform (its rotation turns a velocity into the frame).
    pub attachment: torque_collide::math::Matrix,
    /// The local translation.
    pub translation: [f32; 3],
    /// The yaw in the frame (a Player's, which turns there by its moves alone).
    pub yaw: f32,
}

struct Installed(Rc<dyn GhostBlocks>);

/// Makes `blocks` what fills the block of the records of `vm`'s server.
pub fn install(vm: &mut Vm, blocks: Rc<dyn GhostBlocks>) {
    vm.set_extension(Rc::new(Installed(blocks)));
}

/// What fills the block of the records of `vm`'s server, if anything does.
pub fn of(vm: &Vm) -> Option<Rc<dyn GhostBlocks>> {
    vm.extension::<Installed>()
        .map(|i| Rc::clone(&i.0))
        .filter(|b| b.sections().any())
}
