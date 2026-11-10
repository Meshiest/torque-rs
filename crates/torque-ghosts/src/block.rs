//! The per-ghost block: what the engine's own peers send of a ghost beside its class record, right after it in the same
//! update. A connection carries it only when both peers' manifests list what fills it, so a TGE peer never sees one.
//!
//! **Wire form.** One flag a section, in the order of [`BlockSections`]' fields, then the section's fields when the flag is
//! set. A section that is off on the connection takes no bits at all. Today there is one section, the scene hierarchy's
//! ([`BlockSections::links`]): two groups, each under its own flag and its own mask bit ([`LINK_MASK`], [`POSE_MASK`]):
//!
//! - **link**: `flag(has parent)`; with a parent, `flag(waiting)`, and when not waiting the parent's ghost index at the
//!   connection's ghost-id width and `flag(node)` with a 16-bit node index;
//! - **pose**: the local pose in the parent, quantized (each translation axis a signed 25-bit count of 1/1024 units, the
//!   rotation as the dropped component's index in 2 bits and three 15-bit parts, `flag(scale)` with three 16-bit scale codes,
//!   `flag(snap)`).
//!
//! **Mask bits.** The block owns [`BLOCK_MASK`] (bits 30 and 31) of every ghost's update mask; a class record reads only
//! [`CLASS_MASK`]. TGE's records use bits 0 to 28. A plugin record may not use bits 30 and 31. The guard is
//! `#[derive(GhostRecord)]`, which refuses such a `mask` at compile time; for a hand-written record,
//! [`crate::plugin::PluginClasses::new`] makes a best-effort check, packing it from a view of nothing with only those bits
//! dirty, and refuses a class seen to read or keep them ([`crate::plugin::PluginClassError::BlockMask`]).
//!
//! **Hostile input.** [`GhostBlock::read`] takes at most [`MAX_BLOCK_BITS`] bits, reserves nothing and never panics; the
//! values it reads are whatever the server wrote, which the reader checks before it uses them (a ghost index the client
//! lacks, a code out of range).

use torque_net::NetError;
use torque_net::bits::{BitReader, BitWriter};

use crate::{ChargeFn, pay};

/// The mask bit of the link group (parent, node, waiting).
pub const LINK_MASK: u32 = 1 << 31;
/// The mask bit of the local pose group.
pub const POSE_MASK: u32 = 1 << 30;
/// The bits of a ghost's update mask the block owns.
pub const BLOCK_MASK: u32 = LINK_MASK | POSE_MASK;
/// The bits a class record reads.
pub const CLASS_MASK: u32 = !BLOCK_MASK;

// The block's bits are past every bit TGE's records use (an Item's rotation, bit 28, is the highest).
const _: () = assert!(crate::shapes::item_mask::ROTATION < POSE_MASK);

/// Bits of each translation axis on the wire: a sign and 24 bits of magnitude (the largest code, 10,240,000, fits).
pub const TRANSLATION_BITS: u8 = 25;
/// Bits of each of the rotation's three kept parts.
pub const ROTATION_PART_BITS: u8 = 15;
/// Bits of each scale code.
pub const SCALE_BITS: u8 = 16;
/// Bits of a node index.
pub const NODE_BITS: u8 = 16;
/// The most bits a block with every section on can take, at the widest ghost-id width (32).
pub const MAX_BLOCK_BITS: usize = 1 + (3 + 32 + 16) + 1 + (3 * 25 + 2 + 3 * 15 + 1 + 3 * 16 + 1);

/// The sections of the block a connection carries, as both peers agreed through the manifest. A host keeps it as an extension
/// of its script engine (the plugins that fill a section set their flag), and a connection carries a block only when one
/// section is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockSections {
    /// The scene hierarchy's link and local pose.
    pub links: bool,
}

impl BlockSections {
    /// Whether any section is on (a block follows every record).
    #[must_use]
    pub fn any(self) -> bool {
        self.links
    }
}

/// Where a ghost hangs, as the link group says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkField {
    /// No parent.
    Root,
    /// A parent the connection has no ghost of yet: the record's own pose stands until the link is sent.
    Waiting,
    /// The parent's ghost index, and the node of its shape (`None`: its origin).
    Parent { ghost: u32, node: Option<u16> },
}

/// A local pose as the pose group carries it: whole numbers, as `torque_hierarchy::QuantLocal` has them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PoseField {
    /// Each axis in 1/1024 units.
    pub translation: [i32; 3],
    /// The index of the rotation's dropped component (0 to 3).
    pub dropped: u8,
    /// The other three components' codes.
    pub parts: [u16; 3],
    /// The child's scale codes; `None` for a scale of 1 on every axis.
    pub scale: Option<[u16; 3]>,
    /// The client shows the pose at once instead of blending to it.
    pub snap: bool,
}

/// The block of one update: each group that was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct GhostBlock {
    pub link: Option<LinkField>,
    pub pose: Option<PoseField>,
}

impl GhostBlock {
    /// Whether nothing was sent.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.link.is_none() && self.pose.is_none()
    }

    /// Writes the block for a connection carrying `sections`, with ghost indices `ghost_bits` wide. A section that is off
    /// writes nothing, whatever the block holds.
    pub fn write(&self, w: &mut BitWriter, sections: BlockSections, ghost_bits: u8) {
        if !sections.links {
            return;
        }
        w.write_flag(self.link.is_some());
        if let Some(link) = self.link {
            write_link(w, link, ghost_bits);
        }
        w.write_flag(self.pose.is_some());
        if let Some(pose) = &self.pose {
            write_pose(w, pose);
        }
    }

    /// Reads a block written for `sections` with ghost indices `ghost_bits` wide, charging the bytes it read to `charge`.
    ///
    /// # Errors
    /// [`NetError::Overrun`] when the stream ends inside it, or the charge's error when the budget is spent.
    pub fn read(
        r: &mut BitReader,
        sections: BlockSections,
        ghost_bits: u8,
        charge: &mut ChargeFn,
    ) -> Result<GhostBlock, NetError> {
        let start = r.position();
        let mut block = GhostBlock::default();
        if sections.links {
            if r.read_flag()? {
                block.link = Some(read_link(r, ghost_bits)?);
            }
            if r.read_flag()? {
                block.pose = Some(read_pose(r)?);
            }
        }
        let bytes = (r.position() - start).div_ceil(8);
        pay(charge, bytes.max(1) as u64)?;
        Ok(block)
    }
}

fn write_link(w: &mut BitWriter, link: LinkField, ghost_bits: u8) {
    match link {
        LinkField::Root => w.write_flag(false),
        LinkField::Waiting => {
            w.write_flag(true);
            w.write_flag(true);
        }
        LinkField::Parent { ghost, node } => {
            w.write_flag(true);
            w.write_flag(false);
            w.write_bits(ghost, ghost_bits);
            w.write_flag(node.is_some());
            if let Some(node) = node {
                w.write_bits(u32::from(node), NODE_BITS);
            }
        }
    }
}

fn read_link(r: &mut BitReader, ghost_bits: u8) -> Result<LinkField, NetError> {
    if !r.read_flag()? {
        return Ok(LinkField::Root);
    }
    if r.read_flag()? {
        return Ok(LinkField::Waiting);
    }
    let ghost = r.read_bits(ghost_bits)?;
    let node = if r.read_flag()? {
        // Read in 16 bits: the cast keeps every bit.
        Some(r.read_bits(NODE_BITS)? as u16)
    } else {
        None
    };
    Ok(LinkField::Parent { ghost, node })
}

fn write_pose(w: &mut BitWriter, pose: &PoseField) {
    for t in pose.translation {
        w.write_signed_int(t, TRANSLATION_BITS);
    }
    w.write_bits(u32::from(pose.dropped), 2);
    for p in pose.parts {
        w.write_bits(u32::from(p), ROTATION_PART_BITS);
    }
    w.write_flag(pose.scale.is_some());
    if let Some(scale) = pose.scale {
        for s in scale {
            w.write_bits(u32::from(s), SCALE_BITS);
        }
    }
    w.write_flag(pose.snap);
}

fn read_pose(r: &mut BitReader) -> Result<PoseField, NetError> {
    let mut translation = [0i32; 3];
    for t in &mut translation {
        *t = r.read_signed_int(TRANSLATION_BITS)?;
    }
    // Each read is at most as wide as the type it is cast to: the casts keep every bit.
    let dropped = r.read_bits(2)? as u8;
    let mut parts = [0u16; 3];
    for p in &mut parts {
        *p = r.read_bits(ROTATION_PART_BITS)? as u16;
    }
    let scale = if r.read_flag()? {
        let mut scale = [0u16; 3];
        for s in &mut scale {
            *s = r.read_bits(SCALE_BITS)? as u16;
        }
        Some(scale)
    } else {
        None
    };
    let snap = r.read_flag()?;
    Ok(PoseField {
        translation,
        dropped,
        parts,
        scale,
        snap,
    })
}

#[cfg(test)]
#[path = "block_tests.rs"]
mod tests;
