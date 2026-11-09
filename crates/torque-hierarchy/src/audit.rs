//! A check of every rule the tree keeps, for tests and fuzzing.

use std::collections::BTreeSet;
use std::fmt;

use crate::tree::{Hierarchy, Node, Slot};
use crate::types::ObjId;

/// A rule the tree broke, found by [`Hierarchy::audit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// The slab holds `slots` nodes but the index lists `indexed`.
    IndexCount { slots: usize, indexed: usize },
    /// A slot is neither held by a node nor on the free list.
    SlotLeak,
    /// The index does not point at the object's slot.
    NotIndexed(ObjId),
    /// An object with neither a parent nor children is still held.
    Idle(ObjId),
    /// An object is flagged queued but is not on the queue.
    QueueFlag(ObjId),
    /// A world transform is not finite.
    NotFinite(ObjId),
    /// A local pose is out of range.
    LocalRange(ObjId),
    /// An object has more children or a greater depth than the caps allow.
    PastCap(ObjId),
    /// An object lists a child that is gone.
    ChildGone(ObjId),
    /// A parent and a child disagree about their link or depth.
    Disagree { parent: ObjId, child: ObjId },
    /// An object lists a child twice.
    ChildTwice(ObjId),
    /// An object's descendant count is not the sum over its children.
    SubtreeCount {
        object: ObjId,
        held: usize,
        found: usize,
    },
    /// An object's height is not one past its tallest child's.
    Height(ObjId),
    /// A root has a depth, or more descendants than the cap allows.
    RootShape(ObjId),
    /// An object's parent is gone.
    ParentGone(ObjId),
    /// An object is not in its parent's list of children.
    NotInParent(ObjId),
    /// An object is its own ancestor.
    Cycle(ObjId),
    /// The count of parented objects is wrong or past the cap.
    ParentedCount { held: usize, found: usize },
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Violation::IndexCount { slots, indexed } => {
                write!(f, "{slots} nodes held but {indexed} indexed")
            }
            Violation::SlotLeak => f.write_str("a slot is neither held nor free"),
            Violation::NotIndexed(id) => write!(f, "{id:?} is not indexed at its slot"),
            Violation::Idle(id) => write!(f, "{id:?} has neither a parent nor children"),
            Violation::QueueFlag(id) => write!(f, "{id:?} is flagged queued but not queued"),
            Violation::NotFinite(id) => write!(f, "{id:?} has a world that is not finite"),
            Violation::LocalRange(id) => write!(f, "{id:?} has a local pose out of range"),
            Violation::PastCap(id) => write!(f, "{id:?} is past a cap"),
            Violation::ChildGone(id) => write!(f, "{id:?} lists a child that is gone"),
            Violation::Disagree { parent, child } => {
                write!(f, "{parent:?} and its child {child:?} disagree")
            }
            Violation::ChildTwice(id) => write!(f, "{id:?} lists a child twice"),
            Violation::SubtreeCount {
                object,
                held,
                found,
            } => write!(f, "{object:?} counts {held} below but has {found}"),
            Violation::Height(id) => write!(f, "{id:?} has the wrong height"),
            Violation::RootShape(id) => write!(f, "root {id:?} has a depth or too many below"),
            Violation::ParentGone(id) => write!(f, "{id:?} has a parent that is gone"),
            Violation::NotInParent(id) => write!(f, "{id:?} is not in its parent's children"),
            Violation::Cycle(id) => write!(f, "{id:?} is its own ancestor"),
            Violation::ParentedCount { held, found } => {
                write!(f, "{found} objects have a parent, counted {held}")
            }
        }
    }
}

impl std::error::Error for Violation {}

impl Hierarchy {
    /// Check every rule the tree keeps; linear in the tree's size.
    ///
    /// # Errors
    /// The first rule found broken.
    pub fn audit(&self) -> Result<(), Violation> {
        let held = self.slots.iter().flatten().count();
        if held != self.index.len() {
            return Err(Violation::IndexCount {
                slots: held,
                indexed: self.index.len(),
            });
        }
        if held + self.free.len() != self.slots.len() {
            return Err(Violation::SlotLeak);
        }
        let queued: BTreeSet<Slot> = self.queue.iter().copied().collect();
        let mut parented = 0;
        for (slot, node) in self.slots.iter().enumerate() {
            let Some(node) = node else { continue };
            self.audit_node(slot, node, &queued)?;
            parented += usize::from(node.up.is_some());
        }
        if parented != self.parented || parented > self.caps.total {
            return Err(Violation::ParentedCount {
                held: self.parented,
                found: parented,
            });
        }
        Ok(())
    }

    fn audit_node(
        &self,
        slot: Slot,
        node: &Node,
        queued: &BTreeSet<Slot>,
    ) -> Result<(), Violation> {
        let id = node.id;
        if self.index.get(&id) != Some(&slot) {
            return Err(Violation::NotIndexed(id));
        }
        if node.up.is_none() && node.children.is_empty() {
            return Err(Violation::Idle(id));
        }
        if node.queued && !queued.contains(&slot) {
            return Err(Violation::QueueFlag(id));
        }
        if node.world.iter().any(|v| !v.is_finite()) {
            return Err(Violation::NotFinite(id));
        }
        if node.local.validate().is_err() {
            return Err(Violation::LocalRange(id));
        }
        if node.children.len() > self.caps.children || usize::from(node.depth) > self.caps.depth {
            return Err(Violation::PastCap(id));
        }
        self.audit_children(slot, node)?;
        let Some(up) = node.up else {
            if node.depth != 0 || node.subtree > self.caps.descendants {
                return Err(Violation::RootShape(id));
            }
            return Ok(());
        };
        let Some(above) = self.slots.get(up.slot).and_then(Option::as_ref) else {
            return Err(Violation::ParentGone(id));
        };
        if !above.children.contains(&slot) {
            return Err(Violation::NotInParent(id));
        }
        if self.chain_includes(up.slot, slot) {
            return Err(Violation::Cycle(id));
        }
        Ok(())
    }

    /// The links to the children, and the counts that are sums over them.
    fn audit_children(&self, slot: Slot, node: &Node) -> Result<(), Violation> {
        let mut found = 0;
        let mut height = 0u8;
        for child in &node.children {
            let Some(below) = self.slots.get(*child).and_then(Option::as_ref) else {
                return Err(Violation::ChildGone(node.id));
            };
            if below.up.map(|u| u.slot) != Some(slot) || below.depth != node.depth + 1 {
                return Err(Violation::Disagree {
                    parent: node.id,
                    child: below.id,
                });
            }
            if node.children.iter().filter(|c| *c == child).count() != 1 {
                return Err(Violation::ChildTwice(node.id));
            }
            found += 1 + below.subtree;
            height = height.max(below.height + 1);
        }
        if found != node.subtree {
            return Err(Violation::SubtreeCount {
                object: node.id,
                held: node.subtree,
                found,
            });
        }
        if height != node.height {
            return Err(Violation::Height(node.id));
        }
        Ok(())
    }
}
