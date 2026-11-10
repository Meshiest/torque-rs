//! A controlled child predicted in its parent's frame: the client's control Player hung on a parent (a turret on a tank).
//!
//! The server sends such a Player's packet data in its parent's frame (`torque_ghosts::TailFrame::Parent`): its local
//! translation, its velocity turned into the frame and its yaw there (`PlayerState::frame_yaw`, which its moves turn as they
//! turn its body). The client keeps that local pose ([`ChildFrame::local`]) and, for each move, places the Player where the
//! parent's tick pose and the local pose put it, runs the move in the world as the server does, and takes the new local pose
//! back rounded to the wire's steps ([`hierarchy::wire_local`]), its rotation the frame yaw's. So what it compares with the
//! server is the local state: where the parent went since the server's latest record changes nothing the checksum covers.
//!
//! Each step is the server's own, from the shared crates: the resolve's rounds ([`hierarchy::RESOLVE_ROUNDS`],
//! [`hierarchy::same_pose`]) over a child whose parent or local pose changed, the carry's turn of the velocity
//! ([`torque_game::scene::links::carry_turn`]), a Player's `setTransform` (its position and the yaw of the matrix,
//! [`torque_game::sim::player::yaw_of`]), its own rotation ([`torque_game::scene::links::yaw_rotation`]) and the frame's
//! turn of the velocity ([`torque_game::scene::links::into_frame`]).

use collide::math::{F3, Matrix, mul_v};
use hierarchy::{Local, Pose, RESOLVE_ROUNDS};
use sim::player::PlayerState;
use torque_game::scene::links::{carry_turn, into_frame, out_of_frame, yaw_rotation};
use torque_game::sim::player::yaw_of;

use crate::records::{PlayerWords, TailFrame};

/// Where a frame hangs for one tick: its parent's pose and the node's matrix at rest (`None`: the parent's origin).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Attach {
    pub parent: Pose,
    pub node: Option<Matrix>,
}

impl Attach {
    /// The frame's own transform: the parent's world times the node.
    #[must_use]
    pub fn matrix(&self) -> Matrix {
        hierarchy::compose(&self.parent, self.node.as_ref(), &Local::IDENTITY)
    }

    /// Where `local` puts a child in this frame.
    #[must_use]
    pub fn place(&self, local: &Local) -> Matrix {
        hierarchy::compose(&self.parent, self.node.as_ref(), local)
    }
}

/// The parent frame the predicted Player hangs in, as the server's packet data last said.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChildFrame {
    /// The parent's ghost index and the node of its shape (`None`: its origin).
    pub parent: u32,
    pub node: Option<u16>,
    /// The local pose, rounded to the wire's steps as the server's tree keeps it.
    pub local: Local,
    /// The attachment the Player's state was last placed in. `None` while the state still holds the packet data's own
    /// values (its local translation, yaw and velocity), which the next tick places.
    pub at: Option<Attach>,
}

impl ChildFrame {
    /// The frame `words` say they are in, for a Player of scale `scale`; `None` for the world's, for a Player that sends no
    /// position (the server sends a mounted one in the world's), and for values that are not finite, which no server's tree
    /// holds (the translation and yaw are checked as a local pose, the velocity here).
    #[must_use]
    pub fn read(words: &PlayerWords, scale: F3) -> Option<ChildFrame> {
        let TailFrame::Parent { ghost, node } = words.frame else {
            return None;
        };
        let free = words.free?;
        if !free.velocity.iter().all(|v| v.is_finite()) {
            return None;
        }
        let translation = free.position;
        let local = hierarchy::wire_form(&Local {
            translation,
            rotation: yaw_rotation(words.rot_z),
            scale,
        })
        .ok()?;
        Some(ChildFrame {
            parent: ghost,
            node,
            local,
            at: None,
        })
    }

    /// The server's resolve of the child at `at`: when its parent's pose (or, with `moved`, its own local pose) changed
    /// since the last, the Player placed where its local pose puts it, its velocity turned with it, and the place it kept
    /// (upright) taken back as its local pose while that changes it, up to [`RESOLVE_ROUNDS`] times. A state that still
    /// holds the packet data's values is placed first.
    pub fn settle(&mut self, s: &mut PlayerState, at: Attach, moved: bool) {
        let fresh = self.at.is_none();
        if fresh {
            s.velocity = out_of_frame(&at.matrix(), s.velocity);
            place(s, &at.place(&self.local));
            s.delta_pos = s.position();
            s.delta_rot = s.rot;
        }
        if !fresh && !moved && self.at == Some(at) {
            return;
        }
        self.at = Some(at);
        for _ in 0..RESOLVE_ROUNDS {
            let target = at.place(&self.local);
            if hierarchy::same_pose(&s.obj_to_world, &target) {
                return;
            }
            if let Some(turn) = carry_turn(&s.obj_to_world, &target) {
                s.velocity = mul_v(&turn, s.velocity);
            }
            place(s, &target);
            if hierarchy::same_pose(&s.obj_to_world, &target) {
                return;
            }
            match self.take(s, &at) {
                Some(local) if local != self.local => self.local = local,
                _ => return,
            }
        }
    }

    /// The Player's own move left it where its state is: its new local pose, as the server's tree takes a write that is not
    /// bit for bit its own pose. `Some(true)` when it changed (the end of the step resolves it), `Some(false)` when the
    /// move wrote nothing new, `None` when the tree would refuse the move (the server takes such a child off its parent).
    pub fn moved(&mut self, s: &PlayerState) -> Option<bool> {
        let Some(at) = self.at else {
            return Some(false);
        };
        if s.obj_to_world == at.place(&self.local) {
            return Some(false);
        }
        self.local = self.take(s, &at)?;
        Some(true)
    }

    /// The local pose that puts the Player at its world in the frame, its rotation its frame yaw's.
    fn take(&self, s: &PlayerState, at: &Attach) -> Option<Local> {
        let local = hierarchy::wire_local(
            &s.obj_to_world,
            &at.parent,
            at.node.as_ref(),
            self.local.scale,
        )
        .ok()?;
        match s.frame_yaw {
            Some(yaw) => hierarchy::wire_form(&Local {
                rotation: yaw_rotation(yaw),
                ..local
            })
            .ok(),
            None => Some(local),
        }
    }

    /// `words` (the Player's, from its state) as the server writes them in this frame: the local translation, the velocity
    /// turned into the frame, the frame yaw, and the frame.
    pub fn words(&self, s: &PlayerState, words: &mut PlayerWords) {
        if let Some(free) = words.free.as_mut() {
            free.position = self.local.translation;
            free.velocity = match &self.at {
                Some(at) => into_frame(&at.matrix(), s.velocity),
                None => s.velocity,
            };
        }
        words.rot_z = s
            .frame_yaw
            .unwrap_or_else(|| yaw_of(&collide::math::quat_matrix(self.local.rotation)));
        words.frame = TailFrame::Parent {
            ghost: self.parent,
            node: self.node,
        };
    }
}

/// A Player's `setTransform` without the host: its position and the yaw of `m`.
fn place(s: &mut PlayerState, m: &Matrix) {
    s.place([m[3], m[7], m[11]], [0.0, 0.0, yaw_of(m)], None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::FreeMotion;

    fn words(free: Option<FreeMotion>) -> PlayerWords {
        PlayerWords {
            energy: 0.0,
            flag: false,
            free,
            head_x: 0.0,
            head_z: 0.0,
            rot_z: 0.25,
            translation: [0.0; 3],
            frame: TailFrame::Parent {
                ghost: 4,
                node: None,
            },
        }
    }

    const FREE: FreeMotion = FreeMotion {
        position: [1.0, 2.0, 3.0],
        velocity: [0.0; 3],
        contact: 0,
    };

    /// A Player that sends its position is read in its parent's frame; one that sends none (mounted) or a velocity that is
    /// not finite is not, and writes the world's frame, as the server sends such a Player.
    #[test]
    fn only_a_frame_with_a_position_and_finite_values_is_read() {
        let frame = ChildFrame::read(&words(Some(FREE)), [1.0; 3]).expect("a frame");
        assert_eq!(
            (frame.parent, frame.local.translation),
            (4, [1.0, 2.0, 3.0])
        );
        assert!(ChildFrame::read(&words(None), [1.0; 3]).is_none());
        let wild = FreeMotion {
            velocity: [f32::NAN, 0.0, 0.0],
            ..FREE
        };
        assert!(ChildFrame::read(&words(Some(wild)), [1.0; 3]).is_none());
    }
}
