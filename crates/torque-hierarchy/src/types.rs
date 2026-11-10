//! The values a hierarchy is built from and the trait a host implements.

use torque_collide::math::{F3, Matrix};
use torque_collide::math::{Quat, plain_quat_normalize};

use crate::caps::{
    MAX_SCALE, MAX_TRANSLATION, MAX_WORLD, MIN_SCALE, ParentError, ROTATION_ROW_LENGTH,
};

/// An object's id in the host (a scene object's, or a ghost's index on a client).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjId(pub u32);

/// Where on the parent a child hangs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeRef {
    /// The parent's own origin.
    Origin,
    /// A node of the parent's shape; the host's [`Host::node_matrix`] says where it is.
    Node(u16),
}

/// A child's parent and where it hangs on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParentRef {
    pub object: ObjId,
    pub node: NodeRef,
}

impl ParentRef {
    /// A link to `object`'s origin.
    #[must_use]
    pub fn origin(object: ObjId) -> ParentRef {
        ParentRef {
            object,
            node: NodeRef::Origin,
        }
    }
}

/// An object's world transform (no scale in the matrix, as Torque keeps it) and its own scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub world: Matrix,
    pub scale: F3,
}

impl Pose {
    /// This pose, refused when a value is not finite or the scale is out of range; the matrix's last row is made
    /// `0 0 0 1`.
    pub(crate) fn checked(mut self) -> Result<Pose, ParentError> {
        matrix_check(&self.world)?;
        if !self.scale.iter().all(|s| s.is_finite()) {
            return Err(ParentError::NotFinite);
        }
        if !scale_in_range(self.scale) {
            return Err(ParentError::OutOfRange);
        }
        self.world[12..].copy_from_slice(&[0.0, 0.0, 0.0, 1.0]);
        Ok(self)
    }
}

/// A child's pose in its parent: a translation (scaled by the parent's scale when composed), a unit quaternion in Torque's
/// convention (`x, y, z, w`, as `torque_collide::math::quat_matrix` reads it) and the child's own scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Local {
    pub translation: F3,
    pub rotation: Quat,
    pub scale: F3,
}

impl Local {
    /// No offset, no turn, scale 1.
    pub const IDENTITY: Local = Local {
        translation: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
    };

    /// Whether every value is finite and in range: a translation at most [`MAX_TRANSLATION`] along each axis, a scale in
    /// `MIN_SCALE..=MAX_SCALE` per component and a quaternion whose squared length is within `[0.25, 4]`.
    ///
    /// # Errors
    /// [`ParentError::NotFinite`] or [`ParentError::OutOfRange`].
    pub fn validate(&self) -> Result<(), ParentError> {
        let mut all = self
            .translation
            .iter()
            .chain(&self.rotation)
            .chain(&self.scale);
        if !all.all(|v| v.is_finite()) {
            return Err(ParentError::NotFinite);
        }
        let [qx, qy, qz, qw] = self.rotation;
        let quat_sq = qx * qx + qy * qy + qz * qz + qw * qw;
        let in_range = self.translation.iter().all(|t| t.abs() <= MAX_TRANSLATION)
            && scale_in_range(self.scale)
            && (0.25..=4.0).contains(&quat_sq);
        if in_range {
            Ok(())
        } else {
            Err(ParentError::OutOfRange)
        }
    }

    /// This pose with its rotation made a unit quaternion.
    #[must_use]
    pub(crate) fn normalized(mut self) -> Local {
        self.rotation = plain_quat_normalize(&self.rotation);
        self
    }
}

/// What a reparent keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    /// The object's world pose; its local pose is rewritten.
    World,
    /// The object's local pose (for an object with no parent, its world transform); its world pose changes.
    Local,
}

/// What the hierarchy asks of the engine that owns the objects.
pub trait Host {
    /// The world pose of an object the hierarchy does not hold yet, asked when it first takes the object in.
    fn pose(&self, object: ObjId) -> Option<Pose>;

    /// Node `node` of `object`'s shape in the object's space, as the object's animation leaves it (no scale in the matrix;
    /// the hierarchy scales its translation by the object's scale). `None` when the object has no such node.
    fn node_matrix(&self, object: ObjId, node: u16) -> Option<Matrix>;
}

/// Whether `m` is a transform the hierarchy composes safely: finite, the rows of its rotation part about unit length and its
/// translation within `MAX_WORLD`.
pub(crate) fn matrix_check(m: &Matrix) -> Result<(), ParentError> {
    if !m.iter().all(|v| v.is_finite()) {
        return Err(ParentError::NotFinite);
    }
    let rows_fit = (0..3).all(|r| {
        let length = (m[r * 4].powi(2) + m[r * 4 + 1].powi(2) + m[r * 4 + 2].powi(2)).sqrt();
        ROTATION_ROW_LENGTH.contains(&length)
    });
    if rows_fit && [3, 7, 11].iter().all(|i| m[*i].abs() <= MAX_WORLD) {
        Ok(())
    } else {
        Err(ParentError::OutOfRange)
    }
}

pub(crate) fn scale_in_range(s: F3) -> bool {
    s.iter().all(|v| (MIN_SCALE..=MAX_SCALE).contains(v))
}
