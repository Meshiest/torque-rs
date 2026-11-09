//! The scene hierarchy for torque-rs, server side: any scene object hung on another at a local pose (on the parent's origin
//! or on a node of its shape), the server keeping every child's world where its parent puts it.
//!
//! [`HierarchyPlugin`] adds, to each server:
//!
//! - the `SceneObject` methods `setParent`, `getParent`, `getParentNode`, `getChildCount`, `getChild`, `getRoot`,
//!   `isAncestorOf`, `getLocalTransform`, `setLocalTransform`, `clearChildren`, `deleteRecursive`, `setDeleteWithParent`,
//!   `getDeleteWithParent` and the function `getParentError` ([`natives`]): a server API only. The plugin adds no command or
//!   RPC, so no client can parent anything; a game that lets players attach things does it in its own permission-checked
//!   server command;
//! - the tree ([`torque_hierarchy`]) plugged into the scene's seam ([`torque_game::scene::links`]), reached from Rust through
//!   [`ServerHierarchy::of`];
//! - the prefs `$Pref::Hierarchy::*` ([`HierarchyPrefs`]): the caps and the resolve's budget.
//!
//! It is in the default plugin groups and on when the engine speaks its own protocol; with Torque's protocol it adds nothing
//! (a game that wants it there too adds [`HierarchyPlugin::always`]).
//!
//! ```text
//! %turret.setParent(%tank, 1, "mount0");   // hang it on the tank's mount0 node, where it is now
//! %turret.setDeleteWithParent(1);          // it goes when the tank does
//! %tank.deleteRecursive();                 // or take the tank and everything on it
//! ```
//!
//! # The model
//!
//! - **Any class may be a child or a parent**, Players and vehicles included. A child with a simulation of its own (a Player,
//!   a vehicle used as a turret, an Item) moves in its parent's frame: it ticks after its parent from where its parent's turn
//!   left it, its own moves, physics and limits act on that pose, and what it writes back becomes its new local pose. When a
//!   parent turns, a child's velocity turns with it. Each class keeps its own rules about its rotation (a Player stays
//!   upright and keeps only its yaw; an Item keeps its own upright rotation).
//! - **World and local.** `getTransform`/`setTransform` stay world space for every object; a child's write is turned into its
//!   local pose. `setParent` keeps the world pose by default (`keepWorld` 0 keeps the local one).
//! - **Scale.** A parent's scale scales the translation of its node and of the child's local pose, never the child's own
//!   rotation or scale (Torque's mount rule).
//! - **Mounting** stays its own relation: a mounted object (a SpotLight on a shape too) cannot be parented (`mounted`), and
//!   `mountObject` refuses a parented one. Neither may close a loop with the other: a parent that rides the child through
//!   mounts and parents is a `cycle`, and `mountObject` refuses a rider that carries its mount.
//! - **Props** cannot be children yet (`class`): their physics world owns their pose until props join the hierarchy.
//! - **Objects being removed** (in `onRemove`, or with a delete held until a method returns) are refused as parents and
//!   children (`removing`), so a dying object never gains a link.
//! - **Deleting a parent** takes its children off where they are, except those flagged with `setDeleteWithParent`, which go
//!   with it; `deleteRecursive` deletes an object and everything below it, children first.
//! - **Caps** ([`HierarchyPrefs`], held to the hard caps of [`torque_hierarchy`]): depth 16, children per parent 256,
//!   descendants per root 4,096, parented objects 32,768.
//!
//! # Maintaining
//!
//! **Modules.** [`server`]: the server's state ([`ServerHierarchy`]), its operations, the callbacks and the scene seam's
//! implementation (`written`, `resolve`, `removed`). [`natives`]: the console methods, over [`ServerHierarchy`].
//!
//! **One tick.** A write to an object's transform reaches `written` through `scene::access::set_matrix`: a child's becomes its
//! local pose, a parent's marks its subtree dirty. The scene resolves before each container query, before each object's
//! turn in the process list (so a child ticks from where its parent left it), after each 32 ms step, before a script reads
//! a transform, and after every `After` tick hook (this plugin's [`ServerTick::resolve`] hook, so the children of what a
//! physics step moved follow too). A resolve flushes the tree and writes each moved child through
//! [`torque_game::scene::links::carry`] (the class's own `setTransform`, the bins, its velocity turned), ignoring its own
//! writes; a pose the class adjusted goes back into the tree. Callbacks wait for a point scripts may run: the end of each
//! console method here and the resolve hook.
//!
//! **Invariants**, each with the test that guards it (in `tests/server.rs`):
//! - Every refusal changes nothing and names its reason: one test per reason (`refused_self`, `refused_cycle`, ...,
//!   `refused_removing`, `refused_class`).
//! - Mounts and parents never form a loop between them, SpotLight mounts included: `refused_cycle_through_mounts_both_ways`,
//!   `refused_cycle_through_a_spotlight_mount_both_ways`; `mountObject` returns 0 for a parented rider (`refused_mounted`).
//! - Every write of a transform is heard, a script's field write too: `a_scripts_field_write_moves_the_children`; a move the
//!   tree cannot hold is counted and takes the child off: `a_childs_wild_move_is_counted_and_takes_it_off`.
//! - A child is where its parent puts it whenever anything queries the world: `a_ray_every_tick_hits_a_child_of_a_moving_root`
//!   (600 of 600 ticks).
//! - Deleting a parent leaves its children where they were and drops every link: `deleting_a_parent_detaches_its_children_in_place`
//!   (64 children, world error at most 1e-4).
//! - A recursive delete leaves nothing of the subtree and no dangling link, and a plain delete takes flagged children and
//!   detaches the rest: `a_recursive_delete_of_4096_nodes_leaves_nothing`, `flagged_children_go_with_their_parent`.
//! - A Player child's own simulation runs in its parent's frame: `a_player_child_moves_in_its_parents_frame`.
//! - No client entry point, and nothing in Torque's protocol: `the_natives_are_the_servers_alone`,
//!   `torques_protocol_leaves_the_plugin_out`; the `tge` lines of `just trace-hash` and the wire goldens unchanged.
//!
//! **Seams left for later work.** The network: a child's record still carries its world pose (its class's own mask is set by
//! its `setTransform` when it is carried); the link block, scoping and a client's resolve replace that, and they read
//! [`ServerHierarchy::blend`]. With locals quantized for the wire ([`torque_hierarchy::Hierarchy::quantize_locals`]), a
//! child's own write is rounded and written back: the network work decides how a Player child keeps its bits. Props as
//! children, and rope and joint anchors on children, build on [`ServerHierarchy::world`] and the same resolve.
//!
//! **Checks.** `cargo test -p torque-plugin-hierarchy`; `cargo clippy -p torque-plugin-hierarchy --all-targets -- -D
//! warnings`; `just trace-hash` (every line unchanged); the native effects listing of torque-plugin-loader.

pub mod natives;
pub mod server;

use torque_engine::{
    Effects, EngineBuilder, Plugin, PluginId, ProtocolKind, ServerStage, ServerTick, VmMut,
};

pub use server::{Blend, HierarchyPrefs, LinkError, ServerHierarchy, Stats};
pub use torque_hierarchy::{Keep, Local, NodeRef, ParentError, ParentRef};

/// When the plugin adds the hierarchy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Protocols {
    /// Only when the engine speaks its own protocol (what the default plugin groups add).
    #[default]
    EngineOnly,
    /// With either protocol.
    Both,
}

/// Adds the scene hierarchy to every server ([crate docs](crate)). In the default plugin groups, on in engine mode only.
#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "torque.hierarchy")]
pub struct HierarchyPlugin {
    /// When the hierarchy is added.
    pub protocols: Protocols,
}

impl HierarchyPlugin {
    /// The plugin on whatever protocol the engine speaks (Torque's included).
    #[must_use]
    pub fn always() -> HierarchyPlugin {
        HierarchyPlugin {
            protocols: Protocols::Both,
        }
    }
}

/// The end of each tick: the children of what the `After` hooks moved, then the callbacks.
fn resolve(mut vm: VmMut) {
    if let Some(hierarchy) = ServerHierarchy::of(&vm) {
        hierarchy.resolve(&mut vm);
        hierarchy.run_callbacks(&mut vm);
    }
}

impl Plugin for HierarchyPlugin {
    fn build(&self, _engine: &mut EngineBuilder) {}

    // The protocol is known once every plugin has built.
    fn finish(&self, engine: &mut EngineBuilder) {
        if self.protocols == Protocols::EngineOnly && engine.protocol() != ProtocolKind::Engine {
            return;
        }
        engine.server(ServerStage::Game, |vm, _| {
            ServerHierarchy::install(vm);
            Ok(())
        });
        engine
            .server_script()
            .natives(natives::declared())
            // Its prefs are the hierarchy's limits.
            .with_effects(Effects::ENGINE, |r| {
                r.prefs::<HierarchyPrefs>();
            });
        engine.push(ServerTick::resolve(resolve));
    }
}
