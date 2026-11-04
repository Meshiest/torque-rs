//! `Collision` and `CollisionList`: 64 entries, then `t` and `maxHeight`.

use crate::ObjectId;
use crate::math::F3;

/// `CollisionList::MaxCollisions` (`MAX_COLLISIONS`): `ExtrudedPolyList::end` adds nothing once the list holds 64.
pub const MAX_COLLISIONS: usize = 64;

/// One contact (`Collision`): the object, the point, the polygon's normal, its material, the extruded face it was found
/// with, the face's alignment, and a distance no list writes (the caller's list starts zeroed, as `updatePos` memsets its own).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Collision {
    pub object: Option<ObjectId>,
    pub point: F3,
    pub normal: F3,
    pub material: u32,
    pub face: u32,
    pub face_dot: f32,
    pub distance: f32,
}

/// `CollisionList`: the entries, the earliest time `t` (2.0 when nothing has been found, as `updatePos` sets it) and the
/// highest point among the entries at that time. Lives on the heap (the stack is tight under deep scripts).
#[derive(Debug, Clone, PartialEq)]
pub struct CollisionList {
    pub count: usize,
    pub entries: Box<[Collision; MAX_COLLISIONS]>,
    pub t: f32,
    pub max_height: f32,
}

impl Default for CollisionList {
    fn default() -> Self {
        CollisionList::new()
    }
}

impl CollisionList {
    pub fn new() -> CollisionList {
        CollisionList {
            count: 0,
            entries: Box::new([Collision::default(); MAX_COLLISIONS]),
            t: 0.0,
            max_height: 0.0,
        }
    }

    /// What `Player::updatePos` does before each pass: the count to zero and `t` to 2.0. The entries keep their
    /// contents (a reader looks only below `count`).
    pub fn reset(&mut self) {
        self.count = 0;
        self.t = 2.0;
    }

    /// The live entries.
    pub fn as_slice(&self) -> &[Collision] {
        &self.entries[..self.count.min(MAX_COLLISIONS)]
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Appends an entry (the caller has checked the count against [`MAX_COLLISIONS`]); false when full.
    pub fn push(&mut self, c: Collision) -> bool {
        if self.count >= MAX_COLLISIONS {
            return false;
        }
        self.entries[self.count] = c;
        self.count += 1;
        true
    }
}
