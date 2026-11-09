//! The scene hierarchy for torque-rs: any scene object hung on another at a local pose (on the parent's origin or on a node
//! of its shape), the server keeping every child's world where its parent puts it and, in the engine's own protocol, its
//! clients drawing each child on its parent.
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
//! - the prefs `$Pref::Hierarchy::*` ([`HierarchyPrefs`]): the caps and the resolve's budget;
//! - in the engine's own protocol, the per-ghost block of the server's records ([`torque_ghosts::block`]: each child's link and
//!   local pose) and the manifest entry [`NET_VERSION`], and on each client the block's reading; a client draws children
//!   through `torque_ghost_world::links`.
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
//! - **On the network** (the engine's own protocol): a child's link goes with its own update, its parent named by its ghost
//!   on the connection; until the parent's first record is acknowledged the link waits and the client draws the child at its
//!   record's pose. A parent's motion sends nothing for its children. Local poses are rounded to the wire's steps on the
//!   server, so a client composes the server's world exactly. An object first scoped takes its parent and children with it.
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
//! **The network** (`server`'s `GhostBlocks` for `Shared`, installed with `ServerHierarchy::install(vm, Clients::Links)` in the engine's
//! protocol). A link change (made, moved, taken off, the parent deleted, a wild move) marks the child's block link and pose
//! bits and the record bits its parent's motion took back; a local change (`setLocalTransform`, the child's own move, a pose
//! its class kept) marks the pose bit; a parent's motion marks nothing on a child whose parent every ghosting connection holds
//! (`torque_game::scene::ghost::ghosted_everywhere`, `torque_game::scene::links::carry_quiet`; the bits taken back are held in
//! `State::held` until the link changes). The block asks the connection's ghosts (`torque_net::ghost::ConnectionGhosts`):
//! a parent with an acknowledged ghost is named by its index; one the connection holds but has not acknowledged keeps the
//! link bit (the waiting form); one it does not hold is sent as no parent. Both of the last return the record bits held for
//! the child, so that client's record carries the child's pose, and while any connection lacks the parent its children's
//! carries mark their records as before. When a connection makes a parent's ghost, its children's link and pose bits are
//! marked there (`GhostBlocks::marks_on_ghost`), so a child first sent with no parent gets its link. Every link change moves the revision the source's scope stamp includes. A Player child's own move is rounded to the wire's steps
//! like any local pose.
//!
//! **Network invariants**, in `tests/net.rs` and `tests/net_fuzz.rs`:
//! - A client draws every child within a millimetre of the server's world over 600 ticks with a fifth of the datagrams lost:
//!   `every_childs_client_world_is_within_a_millimetre_of_the_servers_under_loss`; the renderer as well:
//!   `the_renderer_draws_each_child_where_the_server_has_it`.
//! - A moving root's still children cost no update: `a_moving_root_with_64_still_children_sends_no_child_updates` (0 child
//!   updates on 600 of 600 ticks after the first 10).
//! - A child ahead of its parent is drawn at its record's pose, then on its parent:
//!   `a_child_ahead_of_its_parent_is_drawn_at_its_record_pose_until_its_link_resolves`; a child of a deleted parent where the
//!   server left it: `children_of_a_deleted_parent_are_drawn_where_the_server_left_them`.
//! - A parent a connection never ghosts never makes a child wait there: `a_child_of_a_parent_a_client_is_never_sent_is_drawn_at_its_record_pose`.
//! - A client without the plugin is refused at connect: `a_client_without_the_hierarchy_is_turned_away_at_the_manifest`.
//! - A hostile server's blocks never panic a client and its caps hold, and a ghost drawn through a chain of sane records is
//!   drawn at finite numbers whatever links and poses were sent: `hostile_blocks_never_panic_the_client_and_its_caps_hold`
//!   (`TORQUE_FUZZ_HIERARCHY_NET`, default 2,000 operations, the full run 100,000).
//! - A record's mask bits never reach the block's (bits 30 and 31): the guard is `#[derive(GhostRecord)]`, which refuses such
//!   a `mask` at compile time (torque-macros' `tests/ui/ghost_record_block_mask.rs`); `PluginClasses::new` makes a
//!   best-effort check of hand-written records, packing from a view of nothing (torque-ghosts'
//!   `a_ghost_record_that_uses_the_blocks_mask_bits_is_refused`).
//! - A connection that comes to hold a parent it did not marks its children's links there, so a child sent with no parent
//!   (the parent not held yet: scoped later, a slot freed, the relatives' scoping cut by the budget) ends up linked:
//!   `a_child_sent_without_its_parent_is_linked_once_the_parent_is_scoped`,
//!   `a_child_sent_without_its_parent_is_linked_once_a_ghost_slot_frees`,
//!   `a_childs_first_record_ahead_of_its_parents_is_linked_once_the_parent_is_scoped` (`ObjectSource::marks_on_ghost`).
//! - A parent's first record ranks ahead of its children's, its later updates get no more (`first_record_bonus`; torque-net's
//!   `a_first_record_bonus_counts_for_the_first_record_alone`).
//! - `tests/pictures.rs` draws a three-level hierarchy offscreen (`TORQUE_HIERARCHY_PICTURES=<folder>` keeps the PNGs).
//!
//! **Anchors on children.** A rope end hung from a child ghost follows `ClientWorld::world_pose` (through the client's
//! `ClientGhostLookup`): `torque-plugin-rope`'s `tests/hierarchy_anchor.rs`. A joint's anchor on a Player child is where the
//! resolve put it, the props' tick hook writing the tree's pending moves before it reads: `tests/joint_anchor.rs`.
//!
//! **Prediction** (in torque-client and torque-ghost-world; nothing here). A predicting client's collision world places
//! each child for every tick at `ClientWorld::tick_pose_in`: its parents' latest record poses (or where the client runs
//! them) times their nodes at rest times the local pose last sent, and the predicted object's own children where it is
//! before each move. The local pose and the compose are the server's exactly; a parent's motion past its latest record is
//! held, as a moving root's is, never extrapolated. The game's static set is not re-placed. `tests/ride.rs`: a figure riding
//! a deck hung on a moving lift's node is corrected no more often than on the same deck moved as a root along the same path
//! (latencies of 0 and 3 ticks), and never on the same deck held still, and the client's collision world places the deck
//! within a millimetre of the server's pose at the tick of its latest record; the `hierarchy` line of `just trace-hash`
//! hashes that ride's prediction.
//!
//! **Seams left for later work.** Props as children build on
//! [`ServerHierarchy::world`], the same resolve and `ClientWorld::world_pose`.
//!
//! **Checks.** `cargo test -p torque-plugin-hierarchy`; `TORQUE_FUZZ_HIERARCHY_NET=100000 cargo test -p
//! torque-plugin-hierarchy --release --test net_fuzz`; `cargo clippy -p torque-plugin-hierarchy --all-targets -- -D
//! warnings`; `just trace-hash` (the `tge` lines unchanged); the wire goldens; the native effects listing of
//! torque-plugin-loader.

pub mod natives;
pub mod server;

use torque_engine::{
    Effects, EngineBuilder, Plugin, PluginId, ProtocolKind, ServerStage, ServerTick, VmMut,
};
use torque_ghosts::block::BlockSections;

/// The network version of the plugin's per-ghost block: a peer whose block differs has another.
pub const NET_VERSION: u32 = 1;

pub use server::{Blend, Clients, HierarchyPrefs, LinkError, ServerHierarchy, Stats};
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
        // In the engine's own protocol every client draws children from their links: the per-ghost block carries them and
        // the manifest lists the plugin, so a client without it is refused before a block is sent.
        let net = engine.protocol() == ProtocolKind::Engine;
        if net {
            engine.net().version(NET_VERSION);
            // The engine's clients read the block's link section (`torque_client`'s connection asks the engine).
            let mut sections = engine
                .resource::<BlockSections>()
                .copied()
                .unwrap_or_default();
            sections.links = true;
            engine.insert_resource(sections);
        }
        let clients = if net {
            Clients::Links
        } else {
            Clients::Records
        };
        engine.server(ServerStage::Game, move |vm, _| {
            ServerHierarchy::install(vm, clients);
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
