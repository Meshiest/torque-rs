//! Parent and child transforms for any scene object: a forest of objects, each with a local pose in its parent (hung on the
//! parent's origin or on one of its shape's nodes), with caps, cycle and depth checks, dirty roots, a parents-first resolve,
//! keep-world and keep-local reparenting, the wire quantizer for local poses and the checks a client runs on links a server
//! sent. Std only; the host answers poses and node matrices through [`Host`].
//!
//! The tree has no class rules: any object may be a child or a parent. A host that wants one (a class that cannot be
//! parented, a child whose own simulation runs in its parent's frame) enforces it before it calls in.
//!
//! **Scale.** A parent's own scale scales the translation of its node and of the child's local pose, never the child's
//! rotation or scale, so no shear appears. Matrices carry no scale; each object's scale is kept beside its pose.
//!
//! **Deleting.** [`Hierarchy::delete`] detaches a deleted object's children keeping their world pose, except children
//! flagged by [`Hierarchy::set_delete_with_parent`], which it hands back (children first) for the host to delete too.
//! [`Hierarchy::delete_recursive`] (`deleteRecursive` in script) hands back the whole subtree, children first.
//!
//! # Maintaining
//!
//! Module map: `caps` (limits, [`ParentError`]), `types` (ids, [`Local`], [`Pose`], [`Host`]), `compose` (world from local
//! and back), `quant` (wire form of a [`Local`]), `tree` ([`Hierarchy`]), `audit` ([`Hierarchy::audit`]).
//!
//! Data flow. The host calls [`Hierarchy::set_parent`], [`Hierarchy::link`], [`Hierarchy::set_local`],
//! [`Hierarchy::set_world`], [`Hierarchy::set_scale`] or [`Hierarchy::mark_dirty`], which queue dirty roots; [`Hierarchy::flush`]
//! sorts them by depth then id and resolves each subtree parents-first; [`Hierarchy::drain_moved`] lists what changed so the
//! host can update its bins. A client runs a decoded [`QuantLocal`] through [`QuantLocal::check`], [`QuantLocal::to_local`]
//! and [`Hierarchy::link`].
//!
//! Cost. `set_local`, `set_scale`, `mark_dirty` and a root's `set_world` are O(1). `set_parent`, `link`, a child's `set_world`
//! and `delete` first resolve whatever is queued: up to the nodes under the queued roots, one comparison when nothing is.
//! `check_parent` walks only chains (at most the hard depth); heights, depths and counts are kept on attach and detach.
//!
//! Host contract. Call `delete` or `delete_recursive` for an id before the host reuses it (the tree keys on ids). Drain the
//! moved list each tick; it names an object once, and is compacted past twice the held objects, so an undrained list is bounded
//! but loses nothing it should keep. The host's methods take `&self` while the tree is borrowed `&mut`, so a host cannot call
//! back into the tree mid-operation (a `RefCell` around the tree would panic, not corrupt it).
//!
//! Invariants and their tests: no cycle or self link (`self_and_cycle_links_are_refused`, `random_ops_keep_every_cap_and_the_world_pose`);
//! depth, children, descendants and total caps (`the_depth_cap_counts_the_moved_subtree`, `children_descendants_and_total_caps_hold`,
//! `a_subtree_that_got_shorter_fits_where_it_did_not`, `caps_are_clamped_to_the_hard_caps`); a refused change changes nothing
//! (`a_link_with_a_bad_pose_changes_nothing`, `hostile_input_never_panics_or_breaks_a_cap`); keep-world keeps the world pose to
//! 1e-4 units and radians (`random_ops_under_the_default_caps`, `keep_world_rewrites_the_local_pose_and_keep_local_moves_the_object`);
//! parents resolve before children, once per flush, and a budget carries whole roots (`resolve_is_parents_first_and_a_dirty_root_resolves_its_subtree_once`,
//! `a_budget_carries_the_remaining_dirty_roots`); a second run gives the same bits, as no hash map is on a resolve path
//! (`a_second_run_gives_the_same_bits`); deletes leave no dangling link (`a_plain_delete_detaches_keeping_world_and_takes_flagged_children`,
//! `a_recursive_delete_removes_the_subtree_children_first`); the moved list holds an id once (`the_moved_list_names_an_object_once_after_a_delete_and_a_rebuild`);
//! the quantizer's bounds and that any code decodes to a valid pose (`quant::tests`, `quantized_locals_stay_within_the_wire_step`,
//! `a_childs_scale_is_rounded_to_the_wire_form_only_when_locals_are_quantized`); the scale rule (`a_parents_scale_scales_the_childs_translation_only`).
//!
//! Extension points: a new refusal is a [`ParentError`] variant plus a check in [`Hierarchy::check_parent`] (pure, so a client
//! can run it); a new field of [`Local`] needs `compose`, `quant` and [`Local::validate`] changed together; what a node is
//! lives in [`Host::node_matrix`]; a new rule the tree keeps gets a [`Violation`] in `audit`. No class rules in the tree, no
//! hash map on a resolve path.
//!
//! Checks: `cargo test -p torque-hierarchy`; `TORQUE_FUZZ_HIERARCHY=1000000 cargo test -p torque-hierarchy --release --test
//! fuzz`; `cargo bench -p torque-hierarchy`; `cargo clippy -p torque-hierarchy --all-targets -- -D warnings`; `cargo fmt -p
//! torque-hierarchy -- --check`.

mod audit;
mod caps;
mod compose;
mod quant;
mod tree;
mod types;

pub use audit::Violation;
pub use caps::{
    Caps, HARD_CHILDREN, HARD_DEPTH, HARD_DESCENDANTS, HARD_TOTAL, MAX_SCALE, MAX_TRANSLATION,
    MAX_WORLD, MIN_SCALE, ParentError, ROTATION_ROW_LENGTH,
};
pub use quant::{
    MAX_ROTATION_CODE, MAX_TRANSLATION_CODE, QuantLocal, QuantRotation, ROTATION_BITS,
    TRANSLATION_STEPS, quantize,
};
pub use tree::{Flush, Hierarchy};
pub use types::{Host, Keep, Local, NodeRef, ObjId, ParentRef, Pose};
