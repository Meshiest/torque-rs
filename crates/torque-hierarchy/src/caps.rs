//! The limits of a hierarchy and the reasons a link is refused.

use std::fmt;

/// The deepest chain a cap may be raised to (a root is depth 0).
pub const HARD_DEPTH: usize = 32;
/// The most children one parent may have, at the most.
pub const HARD_CHILDREN: usize = 1_024;
/// The most descendants one root may have, at the most; bounds the work of moving a root.
pub const HARD_DESCENDANTS: usize = 16_384;
/// The most parented objects a hierarchy may hold, at the most.
pub const HARD_TOTAL: usize = 131_072;

/// The largest value of a local translation's components, in units.
pub const MAX_TRANSLATION: f32 = 10_000.0;
/// The largest coordinate of a world translation or node translation the hierarchy takes in, in units.
pub const MAX_WORLD: f32 = 1e7;
/// A matrix the hierarchy takes in has rows of its rotation part between these lengths (a true rotation's are 1), so a chain
/// at the deepest hard cap composes without overflowing an `f32`.
pub const ROTATION_ROW_LENGTH: std::ops::RangeInclusive<f32> = 0.5..=1.5;
/// The smallest value of a scale component.
pub const MIN_SCALE: f32 = 1e-4;
/// The largest value of a scale component.
pub const MAX_SCALE: f32 = 1e4;

/// The limits a [`crate::Hierarchy`] holds its tree to. Each is clamped to its hard cap and to at least 1 by
/// [`Caps::clamped`], which [`crate::Hierarchy::new`] applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    /// The longest chain, in links: a child of an object `depth` links below its root is refused.
    pub depth: usize,
    /// The most children of one parent.
    pub children: usize,
    /// The most descendants of one root.
    pub descendants: usize,
    /// The most objects that have a parent, in all.
    pub total: usize,
}

impl Default for Caps {
    fn default() -> Caps {
        Caps {
            depth: 16,
            children: 256,
            descendants: 4_096,
            total: 32_768,
        }
    }
}

impl Caps {
    /// These caps with each value held to `1..=` its hard cap.
    #[must_use]
    pub fn clamped(self) -> Caps {
        Caps {
            depth: self.depth.clamp(1, HARD_DEPTH),
            children: self.children.clamp(1, HARD_CHILDREN),
            descendants: self.descendants.clamp(1, HARD_DESCENDANTS),
            total: self.total.clamp(1, HARD_TOTAL),
        }
    }
}

/// Why a parent link, a local pose or a pose from the host was refused. Nothing changes when one is returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParentError {
    /// An object cannot be its own parent.
    SelfParent,
    /// The new parent is the child or one of its descendants.
    Cycle,
    /// The chain would pass the depth cap.
    Depth,
    /// The parent would pass the children cap.
    Children,
    /// The root of the new parent would pass the descendants cap.
    Subtree,
    /// The hierarchy would pass the parented-objects cap.
    Total,
    /// A value is NaN or infinite.
    NotFinite,
    /// A translation or scale is outside its range, or a rotation is far from a unit quaternion.
    OutOfRange,
    /// The parent has no such node.
    BadNode,
    /// The host has no pose for an object the hierarchy was asked to take in.
    Unknown,
    /// A local pose was set on an object that has no parent.
    NotParented,
}

impl ParentError {
    /// The name scripts see for this reason.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            ParentError::SelfParent => "self",
            ParentError::Cycle => "cycle",
            ParentError::Depth => "depth",
            ParentError::Children => "children",
            ParentError::Subtree => "subtree",
            ParentError::Total => "total",
            ParentError::NotFinite => "notFinite",
            ParentError::OutOfRange => "outOfRange",
            ParentError::BadNode => "badNode",
            ParentError::Unknown => "unknown",
            ParentError::NotParented => "notParented",
        }
    }
}

impl fmt::Display for ParentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ParentError::SelfParent => "an object cannot be its own parent",
            ParentError::Cycle => "the parent is a descendant of the child",
            ParentError::Depth => "the hierarchy would be deeper than its cap",
            ParentError::Children => "the parent has the most children it may",
            ParentError::Subtree => "the root would have more descendants than its cap",
            ParentError::Total => "the hierarchy holds the most parented objects it may",
            ParentError::NotFinite => "a value is not finite",
            ParentError::OutOfRange => "a value is out of range",
            ParentError::BadNode => "the parent has no such node",
            ParentError::Unknown => "the host has no pose for the object",
            ParentError::NotParented => "the object has no parent",
        })
    }
}

impl std::error::Error for ParentError {}
