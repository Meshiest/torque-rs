//! Props for torque-rs: crates, barrels and rubble a server simulates as rigid bodies and every client sees.
//!
//! [`PropsPlugin`] adds
//!
//! - the datablock class `PropData` ([`PropData`]): what a kind of prop collides as (a box or a ball, or the convex hulls of its
//!   shape's collision meshes, [`hulls`]), its mass, friction and bounce, when it sleeps, whether it may be frozen, the shape it
//!   is drawn as;
//! - the console class `Prop` ([`server::NetProp`]) and the server's console functions `propSpawn`, `propDelete`, `propFreeze`,
//!   `propUnfreeze`, `propSleep`, `propApplyImpulse`, `propSetVelocity`, the getters, `propRayCast` and `propCheckPlacement`
//!   ([`natives`]): a server API only. Props have no client entry point: a game's own server commands decide who may spawn
//!   what, checking a placement with [`ServerProps::check_placement`] first;
//! - one persistent Rapier world on the server ([`PropWorld`], on [`torque_physics_rapier`]) stepped after each tick's objects,
//!   its state quantized to the wire's precision every tick ([`quant`]), so a client is sent the server's state exactly;
//! - a collision source on both sides: players, items and projectiles hit props as static boxes, and a player or vehicle that
//!   walks into one pushes it;
//! - the `Prop` ghost class ([`PropGhost`]), its updates ordered and held to a share of each packet, and each client's book of
//!   the props it was sent ([`client::PropBook`]), drawn as shapes, a sleeping or frozen one resting ([`client::draw`]: a
//!   client draws it as a still copy of its shape's static batch, not every frame);
//! - frozen props past a count over a bulk channel ([`frozen`]): a client is ghosted at most `$Pref::Props::FrozenGhosts`
//!   frozen props, and every other frozen prop is sent to it over the `props-frozen` channel (id, datablock, quantized pose,
//!   owner: 27 bytes once) and kept out of its ghosts, so a server may hold thousands of frozen props whatever a connection's
//!   ghost table holds; the client draws them as still copies of their shapes' batches and collides with them as with ghosts;
//! - joints ([`joints`]): tethers (hard or soft), winches, welds, hinges, grapples and holds between props, the world and
//!   players, in the same physics world, made with the server's `joint*` functions (no client entry point), the `Joint` ghost
//!   class ([`JointGhost`]) and each client's book of them ([`JointBook`]); a grapple pulls its player inside the Player's own
//!   tick ([`torque_game::player_tether`]), so a client's prediction replays it exactly; with feature `rope`, networked ropes
//!   that follow joints (`joints::rope`);
//! - props in the scene hierarchy ([`hierarchy`], [`world::Link`]): a prop hung on a dynamic prop joins its body, a prop hung on
//!   anything else is driven to where the tree puts it, an unparented prop leaves at its place's speed, and a prop with a joint
//!   cannot be hung on anything (`getParentError()` says `joints`);
//! - projectiles that push the props they hit, and explosions that push the props around them ([`hits`]);
//! - with [`predict::PropPredictionPlugin`] on a client, the props its own Player holds, stands on or pushes predicted and drawn
//!   through a smoothed correction ([`predict`]), predicted impulses and spawns, and the Player's own shots followed ahead.
//!
//! It is not in `DefaultPlugins`: a game adds it. Props are engine-level: any game with an interactive world can use them.
//!
//! ```
//! use torque_engine::EngineBuilder;
//! use torque_plugin_props::PropsPlugin;
//!
//! let engine = EngineBuilder::new().add_plugins(PropsPlugin).build()?;
//! assert!(engine.is_plugin_added::<PropsPlugin>());
//! # Ok::<(), torque_engine::BuildError>(())
//! ```
//!
//! A server script then describes a kind of prop and spawns some:
//!
//! ```text
//! datablock PropData(Crate)
//! {
//!    shapeFile = "art/crate.dts";
//!    size = "1 1 1";       // the collision box
//!    mass = 20;
//!    friction = 0.7;
//! };
//!
//! %crate = propSpawn(Crate, "0 0 4");
//! propApplyImpulse(%crate, "0 40 0");
//! ```
//!
//! `collision = "shape";` makes a prop collide as its `shapeFile`'s collision meshes (`Collision-1` and on: each mesh one convex
//! hull, at the shape's rest pose and `drawScale`), held together as one rigid body, so an L-shaped desk keeps its notch and a
//! chair tucks under a table. The default is the box of `size` (or the ball of `radius`); `size` is still the box players and
//! shots see. A shape with no collision mesh collides as its bounds; one past a cap ([`hulls::HullCaps`]) or one that does not
//! load, as the `size` box, with a console note.
//!
//! # Limits
//!
//! Props per player (`$Pref::Props::MaxPerPlayer`, 64), per server (`$Pref::Props::MaxProps`, 1,024, at most 4,096) and awake
//! at once (`$Pref::Props::MaxAwake`, 256, at most 1,024), and per remote address of the owners
//! (`$Pref::Props::MaxPerAddress`, 256), whoever asked for the spawn. A client is ghosted at most
//! `$Pref::Props::FrozenGhosts` (256) frozen props; the others go over a channel ([`frozen`]). A spawn past a cap is refused
//! with a console error; past the awake cap the props farthest from every player sleep first. The step is charged to a budget
//! (`$Pref::Props::StepBudget`): a step past it does not run, and the awake cap is lowered until the step fits again. A shape
//! of `collision = "shape"` gives at most [`hulls::MAX_HULLS`] hulls of [`hulls::MAX_HULL_VERTICES`] points each, within
//! [`hulls::MAX_COORD`] units of its origin, and a server holds [`hulls::MAX_SERVER_VERTICES`] points of shapes in all.
//!
//! # Network
//!
//! A server and a client must both have the plugin (the handshake refuses a client without it before any ghost). A
//! TGE-protocol peer is never sent a prop. A prop's updates take at most `$Pref::Props::NetShare` (40%) of each packet, the
//! ones that matter most to that client first ([`server::priority`]); asleep and frozen props cost nothing after their last
//! update. A frozen prop past a client's `$Pref::Props::FrozenGhosts` is not ghosted to it at all: it is sent over the
//! `props-frozen` bulk channel ([`frozen`]), a message of ops that set or remove a prop by its slot, in the channel's share
//! of each packet (a client first hears of 1,700 props in about 50 KB). Which way a prop goes is the server's alone; a client
//! holds a prop both ways for a moment while it changes hands and draws it from its ghost then.
//!
//! # Maintaining
//!
//! **An owner key is the game's text, else the account, else the connection** ([`world::OwnerKey`], `server::owner_key`): a
//! connection's `propOwnerKey` field wins when set; else the account id of the identity the auth provider verified
//! (asked of the host in Rust through `ConnectionSubjects`, never through the script method `getIdentity`, which a game may
//! redefine: `a_script_that_redefines_getidentity_changes_no_ones_props`), so a player who signs in again gets their props
//! back; else the connection (an anonymous player is a new player each time). An account that connects twice at once is one
//! player: the server ends the older session by default (`$Pref::Server::OneSessionPerAccount`), and with that off both
//! connections claim the key, so neither gets orphans back. The three are different variants, so a game's text can never equal an account
//! (`tests/net.rs`: `a_signed_in_player_keeps_their_role_and_their_props_across_a_reconnect`,
//! `a_games_own_owner_key_still_wins_over_the_account`, `anonymous_players_are_new_players_on_every_connection`). The
//! per-address caps are unchanged and count orphans whatever the key.
//!
//! **Modules.** [`natives`]: the server's console functions and the placement checks. [`tick`]: the server's tick hook
//! (owners, statics, pushes, the step, the writeback). [`world`]: the physics world and every rule about props, with no
//! script engine. [`quant`]: the wire's precision. [`hulls`]: a shape's collision hulls (the caps, the server's loader and
//! book, the client's reading of the same file). [`records`], [`data`], [`client`]: what goes on the net (the ghost and
//! datablock records) and what a client does with it. [`server`]: the state, spawning, the collision source and the update
//! priority. [`joints`]: the joint kinds and caps (`mod.rs`), their rules in the world (`world.rs`, an `impl PropWorld`), the
//! `Joint` class and the server's tethers (`server.rs`), the console functions (`natives.rs`), the record (`records.rs`), a
//! client's book and tethers (`client.rs`), and the ropes (`rope.rs`, feature `rope`). [`hits`]: the impulses of a
//! projectile's hit and of an explosion, on both sides. [`predict`]: a client's prediction (`mod.rs`: the config, the shared
//! state, the client functions; `part.rs`: the slot's part, its world, the restore and replay, the drawing, the shots).
//! [`hierarchy`]: what the server reads of the scene hierarchy before each step (`hierarchy::reconcile`) and the rule that
//! refuses a prop with joints as a child ([`hierarchy::PropRule`]); the world's side of a child is `world/children.rs`
//! ([`world::Link`], `impl PropWorld`). [`frozen`]: the `props-frozen` channel: its messages (`messages.rs`), the server's
//! dealing of frozen props and the handover between ghost and channel (`server.rs`, once a tick after the step), and a
//! client's table, the channel's logic and the stills it publishes (`client.rs`).
//!
//! **One tick.** During the process list each grappled Player's own step asks the server's tether source
//! ([`joints::server::ServerTethers`]) for its tether and reports the pull back. After the process list, the hook writes the
//! scene hierarchy's pending children (a grappled Player may hang on a parent that moved), drops props and
//! joints whose objects went, tends owners and orphans, looks for the map's statics every 64 ticks, tells the world where the
//! grappled players are, gathers the players' and vehicles' pushes, and calls [`PropWorld::tick`]: a stopped world is made again
//! (its joints too, in id order), pushes become impulses, grapples whose players went are taken out, each grapple's pull goes to
//! the prop at its far end and each winch reels, the props a joint woke are reported, the world steps (not when nothing moves)
//! under the step budget, each awake prop is quantized and written back, sleepers and the awake cap are applied, and each joint's
//! distance, force, limit and break are read. The hook then writes the record fields of each dirty prop and joint, marks their
//! ghost groups, deletes the props that went and the joints (and ropes) that went with them, and calls `onPropSleep`,
//! `onPropWake`, `onBreak` and the hit props' `onCollision`. Before the step, the projectiles' hits the server's collision
//! source was told of during the process list push their props, and each explosion seen for the first time pushes the props in
//! its reach.
//!
//! **Props in the hierarchy.** Before each step the hook has the hierarchy resolve, then `hierarchy::reconcile` walks the props
//! that changed, parents first (the objects the hierarchy logged as changing a link or a local pose with what hangs below them,
//! the roots frozen, thawed or removed, and the driven props for their targets; the first fit and one after the log overflowed
//! look at every prop), and makes each fit its place. The world indexes its children by kind and root (`world/children.rs`,
//! `PropWorld::set_link` keeps it), a root's parts are settled only when the root moved, and a driven prop still at its target
//! costs a comparison, so a stable tree costs what changed ([`hierarchy::LinkStats`]). The bodies and colliders a fit makes are
//! charged to the step's budget, and a tick past it fits no more (a script that re-parents props every tick is bounded); what was
//! refused is tried again at the next tick (`fits_refused_for_the_budget_land_on_later_ticks`). The changes of a tick are gathered into one
//! set and walked once, a walk past the number of props is a full fit (`a_script_changing_a_large_tree_many_times_in_a_tick_costs_one_bounded_look`),
//! and a change made while the physics world is stopped waits for its rebuild (`a_parent_set_while_the_physics_world_is_stopped_lands_after_its_rebuild`). A prop
//! the world will not put back after a failed change is deleted ([`world::TickReport::dropped`]); a stopped world waits for its
//! rebuild. Each prop: a prop whose parent is a dynamic prop (or a part of
//! one) is attached to that root's body at the chain of local poses from the root down, composed by the tree's own rule, and
//! moved only when a link or a local pose in the chain changed; a prop under anything else is driven (a kinematic body given the
//! resolved world pose as its target, which it reaches inside the step; it pushes dynamic props and is pushed by none); a prop
//! whose parent went is a dynamic body again, at the speed its body gave its point (a part) or its last drive (a driven one).
//! Freezing a root drives its parts where they are; thawing it takes them back at the next tick. After the step each child's
//! state is written to its record **without marking its ghost** (a client draws a child from its parent, so a parent's motion costs
//! the child no update; a new ghost's record carries the pose it has now), and a child's scene transform is the hierarchy's,
//! never written back. A child is never asleep or frozen on the wire, so a client never draws it as resting.
//!
//! **A client tick** (with [`predict::PropPredictionPlugin`]), after the predictor's: the Player's push for the move recorded,
//! the predicted set chosen and the client's world brought to it, the pushes of the tick taken (the game's, the Player's shots),
//! then a restore and replay when the server sent new states (or one step), the drawn poses eased and the predicted states
//! written into the book, which the Player's next move meets.
//!
//! **A datablock changed mid-game.** Each prop's numbers ([`client::ClientProp::params`]) are read from the `PropData` object
//! and keyed on that object's generation (`torque_client::datablocks::DatablockObjects`, moved when a datablock event wrote
//! it); the tick after the events (`client::refresh_datablocks`) reads them again for the props whose datablock moved, at most
//! `MAX_REFRESHES_PER_FRAME` props a tick, and notes where the datablock's collision hulls come from again. The predicted
//! bodies take the new numbers at their next step with their poses and speeds kept. `tests/datablock_refresh.rs`: the book's
//! numbers equal a fresh book's after 100 edits, a resend reads nothing again, a refresh of three ticks' worth takes four ticks.
//!
//! **Props that collide as their shape.** A datablock's `collision` field (`PropCollision`; it is in the datablock record, so
//! network version 4) picks box or shape. For shape, `PropParams` holds `PropShape::Hulls { half, volume }`: `half` is the box
//! that still stands for the prop wherever only a box is asked (`size` or `radius`: the placement checks, the collision source,
//! the prediction's choice of what a Player touches), and `volume` what the hulls hold, which turns a `mass` into a density
//! (`PropParams::with_hulls`). The hulls themselves are an `Rc<PropHulls>` on the [`Prop`](world::Prop)
//! (`PropWorld::spawn_with`), so a world made again, a prop that freezes or thaws and a prop hung on another all build the same
//! compound (`world::body_shape`). *Server:* `server::spawn` reads the datablock and asks `ServerProps::hulls`
//! ([`hulls::HullBook`]) for the hulls of its `shapeFile` at its `drawScale`; the book reads the shape through the engine's own
//! loader (`torque_game::shapes::shapes_of`: the kept set, the read bounds, the script's budget) and takes each collision
//! detail's meshes at the rest pose (`rays::detail_parts`). A shape is read once whatever the number of props, and a datablock's
//! note (no mesh, or refused) prints once. *Client:* the prediction part reads no file itself: it asks the slot's
//! `TickStep::shape` (the connection's shape cache, charged to the parts' budget), a prop whose shape is not read yet waits a
//! tick or two before it joins the predicted set (`Cx::fitted`). Both sides end in `PropHulls::of_parts`, one function, so for one
//! shape the hulls are the same to the bit. *The two agree:* the server writes what it ended up colliding the prop as into the
//! `Prop` ghost's init record (`InitWire::hulls`, the `hullCount`, `hullPoints` and `hullVolume` fields of the server's prop: a
//! `hulls::HullDigest`, none for the datablock's box, which is also what a cap, the server's total or a file it cannot read
//! give), and the client predicts the hulls only if its own build has that digest (`hulls::choose`): the box for none, and for a
//! build that differs or fails (another file under the path, no file, its own budget or caps) the prop is not predicted at all
//! (`State::unfit`, left to the server), never predicted as a shape the server does not have. The datablock's `collision` text is
//! made canonical (`shape` or `box`) by the datablock's preload before it is sent, so the wire's 8 bytes cannot split the two.
//! *Work.* Every step of building hulls is charged: `PropHulls::of_parts` pays for each point it reads and for the volume's build
//! (the server to the script's budget, the client to the tick's budget for parts, where a refusal is a wait), each body made or
//! made again (a spawn, a freeze or thaw, a world made again) is charged to the next steps' budgets (`PropWorld::built_work`: at most half a step's budget a step, so a script
//! that makes hundreds in one pass slows the steps after it and loses none; one console line a tick when the work is past a quarter of
//! the budget; the debt is held to `world::BUILD_DEBT_STEPS` (4) budgets, which eight steps pay off, and the excess is forgiven and
//! counted in `PropWorld::forgiven_work`). A spawn is never refused for it, so the most build work a pass of a script can do
//! synchronously is `$Pref::Props::MaxProps` (1,024) times the most one shape costs (32 hulls of 64 points, about 20,000 units): the
//! props' own caps bound it, and `MaxPerPlayer` bounds what one client's commands can make. The physics step charges a contact pair by the hulls of both
//! shapes. A client keeps at most 256 shapes' hulls and starts over past it. The physics crate's `Shape::Compound` makes each hull as a `Shape::Hull` and places it at its middle (a
//! hull that cannot be made is its box; a compound of more than `Limits::max_compound_hulls` hulls is the box of all its points).
//!
//! **Frozen props over the channel** (`frozen::server::sync`, the last thing the tick hook does). Needs the hub the server sends
//! bulk channels through and the scope control (`engine.net().scope_control()`); a server without a channel for a connection (a
//! client of Torque's protocol alone, a connection whose channels are not on) deals it nothing and ghosts it every prop as before.
//! `Snapshot::refresh` keeps the frozen props that may go over the channel (frozen, no parent or child in the scene hierarchy, no
//! joint) from the world's log of touched props, looking at those only (every frozen prop when the hierarchy's links changed),
//! and appends what changed to a log each client reads from its own place; no more than
//! [`frozen::server::DEAL_STEPS_PER_TICK`] props are looked at a tick over all clients (they take turns first), so a client that
//! joins, or a mass thaw, is spread over the ticks. A client is ghosted its first `$Pref::Props::FrozenGhosts` frozen props; the
//! rest go over the channel and are hidden from its ghosts. A prop that no client was ever ghosted (every one a new client hears
//! of, every one that is frozen when it first exists) is hidden at once, also when its deal was put off, unless the client
//! acknowledged a ghost of it meanwhile; one that may have a ghost keeps it until the channel's message is acknowledged
//! (`ChannelHub::queued_message_len`) and is hidden then. A prop that goes back (thawed, hung, given a joint) is shown again and
//! its remove op is sent when the client acknowledged its ghost (`GhostLink::ghost_index_on`, asked about a bounded number a
//! tick: [`frozen::server::ACK_CHECKS_PER_TICK`]). A client holds a prop both ways during either handover and draws it from its
//! ghost ([`frozen::client::FrozenTable::stills`] leaves out the slots that have ghosts, and so does the collision source), so
//! every tick has one picture of it. A message the hub refuses is tried again at the next tick; nothing of a client's state moves
//! until its message was queued. The client's tick reads the datablocks the table's props name (and lets go of those none names;
//! one the client lacks is not remembered), makes a still of each prop it shows from the table and hands them to the renderer
//! (`engine.net().stills()`: the client's world draws them as it draws resting ghosts, as copies of one static batch a shape). The
//! server's half is written against `frozen::server::Port`, so its work is counted without a network
//! (`a_tick_costs_what_changed_at_4096_props_and_64_clients`).
//!
//! **Invariants**, each with the test that guards it:
//! - The state written back is the wire's, so a client restores the server's bits and native equals wasm32:
//!   `every_awake_state_is_written_back_at_the_wires_precision`, `a_quantized_state_quantizes_back_to_the_same_state`, and the
//!   `props-drop` line of `just trace-hash`.
//! - A prop of hulls rests in its shape's notch where a box would stand on top, keeps the datablock's mass, and sleeps and stays
//!   still where it rests: `a_prop_with_an_l_shaped_hull_lets_another_sit_in_its_notch_where_a_box_would_not`,
//!   `a_crate_dropped_on_the_l_arm_rests_on_it_and_a_hulled_prop_keeps_its_mass`,
//!   `a_hulled_prop_asleep_where_it_rests_stays_still`.
//! - A server and a client hold the same hulls for a shape (file, node poses, draw scale) and refuse the same shapes past a cap,
//!   and a hostile file (counts, points, bytes) is refused or held and never panics:
//!   `a_server_and_a_client_hold_the_same_hulls_for_a_shape`, `a_shape_past_a_cap_is_refused_the_same_way_on_both_sides`,
//!   `a_hostile_shape_file_is_refused_cleanly_whatever_its_counts_say`,
//!   `hostile_collision_meshes_are_refused_or_held_and_never_reach_the_world_unchecked`.
//! - The caps and the fallbacks hold, each with a note: `the_caps_refuse_too_many_hulls_too_many_points_and_points_that_are_no_numbers`,
//!   `the_server_holds_the_points_of_all_its_shapes_to_a_total`,
//!   `a_datablock_of_shape_collision_gets_its_hulls_and_every_fallback_is_the_box_with_a_note`. A datablock that does not ask is
//!   untouched (the `props-drop` line of `just trace-hash`), and a predicting client steps a prop with the hulls the server made:
//!   `a_predicting_client_steps_a_prop_it_touches_with_the_hulls_the_server_made`.
//! - No script text is built from what a player controls (objects are made with `Vm::new_object`, callbacks called natively):
//!   `no_script_text_is_built_from_what_a_player_controls`.
//! - Caps per owner, per address and per server hold whoever asks: `the_caps_refuse_the_65th_for_a_player_and_the_1025th_for_the_server`,
//!   `the_players_of_one_address_share_its_cap_whatever_their_keys`, `reconnecting_from_one_address_does_not_start_a_new_quota`,
//!   `an_owner_with_no_readable_address_is_capped_under_its_connection`.
//! - A share group's priority stays below a kill's: `a_pending_kill_goes_before_a_share_update_of_any_priority` (torque-net).
//! - A joint's anchor on a Player is where the scene hierarchy puts the Player, the tree's pending moves written first:
//!   `a_grapple_from_a_player_on_a_moving_platform_measures_from_where_the_platform_puts_it` (torque-plugin-hierarchy's
//!   `tests/joint_anchor.rs`). On a client a joint's ropes follow their ghost ends through the rope plugin, resolved the same way.
//! - A prop put to sleep where it rests ([`PropWorld::sleep`], `propSleep`: a map's furniture as it loads) is not stepped and
//!   sends nothing until a push wakes it: `a_prop_put_to_sleep_where_it_rests_holds_still_until_a_push_wakes_it`, and the
//!   level demo's `the_props_start_asleep_where_the_model_put_them_and_nothing_moves_while_no_one_touches_them`.
//! - A hold carries its prop within a few centimeters of its point through a walk and a turn, upright, never asleep; it pulls
//!   nothing and only its holder's collision passes through its prop ([`server::ServerSource`] and [`client::ClientSource`]
//!   name the holder, torque-collide's `a_shape_lets_only_the_mover_it_names_pass`); one holder a prop and one prop a holder;
//!   a holder that goes lets go: `a_hold_carries_its_prop_upright_at_its_point_through_a_walk_and_lets_go_when_deleted`. A
//!   frozen prop cannot be held, freezing breaks a hold, a prop stuck past [`joints::HOLD_BREAK`] (making less than [`joints::HOLD_CATCH_UP`] of the ground its speed allows toward its point in a step, so a view flicked from straight down to straight up keeps its hold: `a_view_flicked_from_straight_down_to_straight_up_keeps_the_hold_and_the_prop_catches_up`) breaks it, and the drive
//!   keeps a held prop out of other players' boxes: `a_hold_breaks_when_frozen_or_stuck_and_keeps_out_of_other_players`,
//!   `the_holder_alone_passes_through_its_held_prop`. Every pusher's box (a player's, a vehicle's) is a frictionless kinematic
//!   body of the props' world, placed where its box is and never given a speed, so no prop (shoved by a held one, let go
//!   inside one, fallen on one) stays inside a player: `a_held_prop_shoves_a_free_one_into_no_players_box`. The
//!   holder's client predicts the held prop from each move's box, facing and head pitch ([`world::PlayerAt::of_box`] and
//!   [`world::PlayerAt::looking`], read the same way as the server's). The pose is [`world::PlayerAt::hold_point`]: the offset is
//!   where the prop hangs with the view level; it swings about the holder's eyes ([`world::HOLD_PIVOT`] of its height) with the
//!   view's elevation, then moves straight ahead until its box is [`joints::HOLD_CLEARANCE`] clear of the holder's box (at any
//!   height, so it moves smoothly). `a_held_prop_stays_clear_of_its_holders_box_at_every_pitch_and_heading` and
//!   `the_view_raises_and_lowers_a_held_prop_along_the_view` (`world_tests.rs`) check both, and
//!   `a_held_prop_follows_the_view_and_never_presses_into_its_holder_on_a_walk_and_turn` (`tests/joints.rs`) walks, turns and
//!   tilts the view in the world. The level demo's office: `a_click_picks_up_the_chair_it_looks_at_carries_it_on_a_walk_and_a_second_click_drops_it`.
//! - A prop hung on a non-prop or a frozen prop is a kinematic body within a millimeter of the pose the tree resolved, every tick of
//!   600 under a parent that slides and turns, and a prop resting on it is carried:
//!   `a_kinematic_child_is_within_a_millimeter_of_its_resolved_pose_every_tick`,
//!   `a_kinematic_child_pushes_a_dynamic_prop_and_a_prop_resting_on_it_is_carried` (`tests/hierarchy.rs`).
//! - A stable tree costs no fit and one change costs one (900 parts: 0 props fitted in 30 ticks, 1 for a local pose set), and a
//!   large body's parts come and go in proportion to their number:
//!   `a_stable_tree_of_nine_hundred_parts_costs_no_fit_a_tick_and_one_change_costs_one`,
//!   `deleting_the_parts_of_a_large_body_and_then_the_body_costs_in_proportion`.
//! - What is hung on a body that is then hung on, frozen or removed leaves with it: `a_body_hung_on_something_takes_its_parts_with_it_as_driven_props`.
//!   A prop frozen when it was hung is frozen again when let go: `a_prop_that_was_frozen_when_it_was_hung_is_frozen_again_when_let_go`.
//!   A body that loses a part is woken: `a_part_moved_to_another_root_wakes_the_root_it_left_as_well_as_the_new_one`.
//! - A part's pose in its root's frame is bit for bit what it was at the join, in the tree and in the physics world, over 600 ticks of
//!   crates dropped on the body and shoves; its mass is the body's: `a_compound_childs_local_pose_never_drifts_under_load`.
//!   Parts and driven props are placed by their own parent's kind (chains, non-prop parents):
//!   `a_chain_of_props_and_other_objects_places_each_prop_by_its_own_parent`.
//! - An unparented prop leaves within 1% of its parent's point velocity (a part: the body's speed at its point, a driven one: its
//!   last move): `an_unparented_prop_leaves_at_its_parents_point_velocity`,
//!   `a_prop_unparented_from_a_turning_parent_leaves_at_the_speed_its_place_had`,
//!   `a_part_unparented_leaves_at_the_speed_its_roots_body_gave_its_point`.
//! - A prop with a joint or a hold is refused as a child (`joints`, nothing changes, its joint stays whole), and a child takes no
//!   joint (an existing hold or joint is never broken by a parent: the parent is refused first):
//!   `a_prop_with_a_joint_is_refused_as_a_child_and_a_child_takes_no_joint`.
//! - Freezing, deleting and a world made again keep every child whole: `freezing_a_root_turns_its_parts_into_driven_props_and_thawing_it_takes_them_back`,
//!   `deleting_a_root_sets_its_parts_free_where_they_are_and_a_deleted_part_leaves_its_root_whole`,
//!   `a_stopped_physics_world_is_made_again_with_its_parts_and_driven_props`.
//! - A child is never drawn as resting, and a client finds it where its parent's latest record and its local pose put it (the
//!   prediction leaves it and the props that carry others to the server): `a_child_of_a_sleeping_root_is_never_drawn_as_resting_and_the_root_is`,
//!   `a_client_finds_a_child_prop_where_its_parent_puts_it_and_never_draws_it_resting` (`tests/hierarchy_net.rs`); the
//!   `hierarchy-props` line of `just trace-hash` (engine mode) is equal native and wasm32.
//! - A poisoned world is made again: `a_stopped_world_is_made_again_from_the_props_states`,
//!   `a_prop_lost_when_the_physics_world_is_made_again_is_deleted_everywhere`.
//! - 2,000 frozen props reach a client as 256 ghosts and 1,744 table entries, each drawn once and collided with once, none
//!   sent both ways, no ghost made for a prop that is hidden at once:
//!   `two_thousand_frozen_props_reach_a_client_as_ghosts_under_the_threshold_and_over_the_channel` (`tests/frozen.rs`); a
//!   client that joins later is dealt what is there: `a_client_that_joins_later_is_dealt_the_props_that_are_there`.
//! - A prop changes between a ghost and the channel, both ways, on a clean and on a lossy link, with exactly one picture on
//!   every tick; a deleted one goes from the table and the stills; a prop the hierarchy holds is a ghost:
//!   `a_prop_changes_between_a_ghost_and_the_channel_without_a_frame_drawn_twice_or_not_at_all`,
//!   `a_prop_that_is_frozen_and_hung_on_a_parent_is_a_ghost_and_one_set_free_is_sent_again`.
//! - A client's prediction collides with the props the channel gave it:
//!   `a_figure_walks_into_frozen_props_sent_over_the_channel_and_stops_where_the_server_stops_it`
//!   (`tests/frozen_predict.rs`).
//! - A server's bytes never panic a client or leave a value out of range, a message is taken whole or not at all, and the
//!   table holds one prop a slot (at most [`world::MAX_PROPS`]):
//!   `hostile_bytes_never_panic_and_never_leave_a_value_out_of_range`, `a_table_holds_one_prop_a_slot_whatever_it_is_sent`,
//!   `every_field_is_checked_and_a_refused_message_gives_no_op`, `a_message_is_held_to_its_size_and_its_count`
//!   (`tests/frozen_messages.rs`, `TORQUE_FUZZ_PROPS` for more).
//! - A client with no channel, and one that is Torque's alone, is sent what it always was:
//!   `frozen_props_change_nothing_for_a_client_that_is_torques_alone_or_has_no_channel`.
//! - A client that leaves takes what was hidden from it: `a_client_that_leaves_takes_its_hidden_props_with_it`.
//! - A tick costs what changed: nothing when nothing did, one step a client for one thaw, and a mass thaw asks about at most
//!   [`frozen::server::ACK_CHECKS_PER_TICK`] ghosts a tick: `a_tick_costs_what_changed_at_4096_props_and_64_clients`.
//! - A server cannot grow a client's datablock cache by naming ids it never sent:
//!   `a_server_naming_datablock_ids_it_never_sent_cannot_fill_the_tables_cache`.
//! - A hide that finds the scope control full leaves its prop a ghost, with one console line and a count:
//!   `ScopeControl::refused` (torque-engine's `an_audience_cannot_take_the_room_of_the_others_and_a_refusal_is_counted`).
//! - This crate has no client entry point: `a_client_sending_the_old_spawn_command_gets_nothing`, and for joints
//!   `the_natives_refuse_bad_ends_and_hold_values_to_their_ranges` (its client commands do nothing).
//! - A joint goes with its prop (or a grapple with its player), stays through a freeze, and a world made again takes every joint
//!   again in id order: `a_joint_goes_with_its_prop_and_stays_through_a_freeze_and_a_new_world`,
//!   `a_grapple_tethers_its_player_and_pulls_its_prop_back`,
//!   `a_joint_reaches_every_client_with_its_ends_as_their_own_ghosts_and_goes_with_its_prop`.
//! - Joint caps per prop, per owner, per address and per server hold whoever asks, and an owner's joints follow its props'
//!   orphan rules: `the_caps_refuse_the_17th_on_a_prop_the_65th_for_a_player_one_past_an_address_and_the_1025th_for_the_server`,
//!   `reconnecting_from_one_address_does_not_start_a_new_joint_quota`,
//!   `a_leaving_owners_joints_are_orphans_and_expire_with_its_props`,
//!   `hostile_joint_calls_never_panic_the_server_and_the_caps_hold`.
//! - `jointCheck` reads a player at an end where the server has it and refuses one that is not the asking client's own control
//!   object, or a prop someone else owns, unless the game allows it, and reach-checks a hinge's point:
//!   `a_player_may_only_grapple_itself_and_only_where_the_server_has_it`,
//!   `a_client_may_join_only_its_own_props_or_no_ones_and_a_hinge_its_point_in_reach`.
//! - A joint's script values are held to their ranges (lengths, rates, forces, springs, limits, motors), and its points and
//!   offsets refused when they are not finite or lie past the world's bound, before the physics world sees them (which checks
//!   them again); every value a client decodes is held to its range:
//!   `hostile_joint_values_are_held_and_never_reach_the_physics_world_unchecked`, `ends_that_are_not_two_things_are_refused`,
//!   `hostile_values_are_held_to_their_ranges_on_decode`, `a_hostile_servers_joint_records_never_panic_the_client`.
//! - A player has one grapple, and the server's tether, its pull and a client's prediction find the same one (a client whose
//!   book holds two gives none): `a_player_has_one_grapple_and_an_offset_past_the_world_is_refused`,
//!   `a_clients_book_with_two_grapples_on_one_player_gives_it_no_tether`.
//! - A grapple's tether is the server's bit for bit on the predicting client (its point, length and spring travel as whole
//!   floats, a prop end is read from the quantized state with [`joints::point_on`] on both sides):
//!   `a_grapple_gives_the_clients_prediction_the_servers_tether_exactly` and the `props-joints` line of `just trace-hash` (0
//!   corrections). A Player with no tether runs Torque's step (the host's tether is none by default, and the step then skips
//!   it): every other line of `just trace-hash` unchanged. A tether that is not finite, or on a mounted player, changes
//!   nothing: torque-sim's `a_tether_that_is_not_finite_or_a_mounted_player_changes_nothing`.
//! - A callback (`onBreak`, `onPropSleep`, `onPropWake`) that stops on an error is printed, and the ones after it still run:
//!   `a_callback_that_stops_on_an_error_is_printed_and_the_ones_after_it_still_run`. A physics call that fails where no caller
//!   is there to tell is printed by the tick ([`world::PropWorld::take_problems`]); a freeze the world refuses changes nothing:
//!   `freezing_in_a_stopped_world_changes_nothing_and_the_new_world_keeps_the_joints_whole`.
//! - A joint's break force is compared with the force it holds at the end of a tick (Rapier keeps only the last solver
//!   iteration's impulses), so a knock within one tick does not break it:
//!   `a_joint_pulled_past_its_break_force_breaks_and_holds_nothing`. A weld gives about 2 mm in its first loaded tick, then holds
//!   with no creep: `a_weld_drifts_at_most_a_millimeter_over_600_ticks_under_load`.
//! - A sleeping prop a joint changes wakes (a sleeper otherwise settles back where it lay, and a winch would never lift it):
//!   `a_winch_reaches_its_target_within_two_percent_of_the_planned_time`, `a_hinge_holds_its_limits_within_half_a_degree`.
//! - A projectile's hit pushes its prop along the shot, held to the cap, and both `onCollision`s run; without plugin shapes a
//!   projectile is Torque's: `a_projectile_pushes_a_prop_along_its_flight_held_to_the_cap` (`--test predict`) and every TGE
//!   line of `just trace-hash` unchanged. An impulse at the center leaves a body's spin as it is (torque-physics-rapier), so the
//!   pushes of every earlier line hash as before.
//! - A grapple's pull on a prop is the momentum of the pair's reduced mass ([`joints::grapple_pull`]), the same on the server and
//!   in a client's prediction, so a light prop is not flung past its Player; toward a fixed end (a point, a frozen or missing
//!   prop) it is the Player's own, and nothing moves (the `props-joints` line unchanged). Its break force counts the Player's
//!   momentum whatever the far end is ([`joints::grapple_momentum`]):
//!   `a_grapples_pull_moves_its_prop_by_the_pairs_reduced_mass_and_counts_the_players_momentum`. The props a client predicts step inside the slot's move loop with its Player (a
//!   correction replays both), so a held prop's prediction holds in either protocol (one packet a tick in the engine's own,
//!   a correction a packet at worst): `a_held_prop_on_a_long_walk_is_drawn_smoothly_where_it_belongs` in both
//!   (`TORQUE_NET_PROTOCOL=engine`); the slot's order: torque-client's
//!   `a_correction_runs_a_part_from_the_ack_with_each_move_after_the_control_objects`. Inside the loop the Player meets only the
//!   predicted props' shapes read again (by slot, [`client::PropBook::in_slot`]; torque-collide's
//!   `a_refresh_reads_only_the_keys_given_again`), each charged to the tick's budget for parts, so a book of thousands costs
//!   no more a move; the whole client tick is timed by `replay-bench`. A hit's push is made at the move of the hit, which is
//!   stepped again with it (drawn in the frame of the hit): `the_shooter_sees_a_predicted_prop_move_when_its_shot_arrives`.
//! - The client's prediction changes nothing of a client without [`predict::PropPredictionPlugin`] (the book's predicted
//!   states stay none, so its collision source and tethers read the server's): the `props-drop` and `props-joints` lines
//!   unchanged. With it, the client's prop prediction gives the same bits native and wasm32: the `carry` and
//!   `projectile-props` lines.
//! - A restore takes the server's state exactly and replays only the moves past the ack, with the pushes and pulls of each move
//!   as that move made them; what is drawn never jumps by more than the server's own prop moves:
//!   `a_held_prop_on_a_long_walk_is_drawn_smoothly_where_it_belongs`,
//!   `riding_a_sliding_prop_and_pushing_a_row_is_predicted_no_worse_than_not_predicting`.
//! - A client's world holds the other players' and the vehicles' boxes as the server places them (frictionless kinematic bodies at
//!   their ghosts' latest records, in ascending id order, [`PropWorld::set_bystanders`]; they push nothing: a body's own speed is
//!   the server's alone, and a player in a crouch, prone or swim pose is boxed standing, the client's player data carrying no pose
//!   box), so a prop pushed toward one stops at it: `a_prop_pushed_toward_another_player_stops_at_it_as_on_the_server` (which
//!   also fails without the boxes, [`predict::PredictConfig::bystanders`] off). A box is left out and counted when it is not a body's
//!   (not finite, or a half-extent past [`world::MAX_PUSHER_HALF`]: [`PropWorld::box_rejected`]); one the physics world refuses
//!   is counted ([`PropWorld::box_refused`]) and noted, and tried again at the next tick; a stopped world is counted apart
//!   ([`PropWorld::box_stopped`]): `a_box_the_physics_world_refuses_is_counted_noted_and_tried_again`,
//!   `a_hostile_box_is_left_out_and_counted_and_a_stopped_world_is_counted_apart`,
//!   `bystanders_are_made_in_ascending_id_order_among_the_pushers`; a box that turns hostile loses its old body:
//!   `a_bystander_whose_box_turns_hostile_loses_its_body`. A hold made against a wall, the holder's own box solid to
//!   its prop, neither throws the prop, jitters it nor moves the holder:
//!   `a_hold_made_against_a_wall_neither_explodes_nor_jitters_nor_pushes_the_holder` (`--test joints`).
//! - Predicted impulses touch only predicted props, are capped a tick and in size, are never sent, and one the server does not
//!   make eases out: `predicted_impulses_are_capped_and_never_sent`,
//!   `a_predicted_impulse_the_server_never_makes_eases_out_with_no_pop`,
//!   `a_predicted_impulse_the_server_makes_too_moves_at_once_and_settles`; the Player's own shots push predicted props at once:
//!   `the_shooter_sees_a_predicted_prop_move_when_its_shot_arrives`,
//!   `an_explosion_pushes_the_predicted_props_in_its_reach_at_once`.
//! - What a client keeps is bounded whatever the server sends or the ack does: the pushes kept for replays (eight a move over
//!   the longest replay): `the_push_log_drops_acknowledged_moves_and_holds_its_cap_however_the_ack_stalls`; the map's pieces
//!   are kept before the props around the predicted ones, which are the nearest 64 (compile-time checks in `part.rs`), so a
//!   pile of props never crowds the floor out; a predicted spawn is adopted by its ghost when the ghost comes, once:
//!   `a_predicted_spawn_is_adopted_by_its_ghost_once_and_one_never_confirmed_fades`; a shot segment that is not finite meets
//!   nothing: `a_shot_segment_that_is_not_finite_meets_no_prop`. A replay is charged to the slot's tick budget for parts and a
//!   tick past it falls back to interpolation.
//! - A projectile's push is read from its datablock, kept with the hit, so a projectile deleted at its hit still pushes; an
//!   explosion pushes the props there are when it went off (the projectiles' explosions are watched, not looked for):
//!   `a_projectile_gone_at_its_hit_pushes_all_the_same_and_an_old_explosion_pushes_no_new_prop`.
//! - The replay of 8 props through 46 moves fits 4 ms natively and 10 ms in wasm32 (`replay-bench` of torque-trace-hash, under
//!   the bench lock).
//! - A rope that shows a joint follows it and pulls on nothing:
//!   `a_joint_rope_at_its_limit_lies_straight_within_a_millimeter_and_a_slack_one_hangs` (feature `rope`); another plugin's
//!   driver under the joints' name is warned about: `another_plugins_driver_under_the_joints_name_is_warned_about`.
//! - Speed: the collision sources answer a revision ([`world::PropWorld::revision`], [`client::PropBook::revision`]) that
//!   takes a new value at every spawn, removal, pose change, record heard and prediction change, so every Player's, Item's and
//!   vehicle's phase of a tick shares one shape set; a change that moves a prop's slot or pose without a new revision leaves
//!   movers meeting the old pose. The hook walks the scene's objects once for the pushers and the players, and writes a prop's
//!   record fields with the object looked up once ([`torque_game::scene::access::set_fields`]). The benches:
//!   `cargo bench -p torque-plugin-props --bench step` and torque-trace-hash's `server` (64 AIPlayers among 1,024 props).
//!
//! **The scope gate** (`frozen::server`'s `step`, `rewatch_scope`). A prop goes over a client's channel only if the connection's
//! scope takes it (the query the ghost manager uses, asked through `GhostLink::scopes`, many props in one call) and no other
//! scope-control user keeps it from the connection (`ScopeControl::is_hidden_by_other`). A prop the gate refuses is in the
//! client's `unseen` set and is not sent; one it holds that leaves is removed at once, and one that enters is sent, as a ghost
//! comes and goes. It is incremental: the scope control's change log (read from `FrozenSync::scope_cursor`; all of it when the log
//! forgot some) puts the props it names on the client's list, and when the connection's scope can vary by itself
//! (`GhostLink::scope_varies`, a scope object that moves) a share of at most [`frozen::server::GATE_SWEEP_PER_TICK`] props a tick
//! over all clients is looked at again in turn. A server with nothing changed and a fixed scope looks at nothing. A connection whose scope is not in place yet (ghosting off, no scope object) takes nothing but what is always in scope, and counts as varying: when it settles, every prop it holds or is kept from is looked at once more. The tests: a
//! prop out of a client's scope is never sent or ghosted, one hidden by another user is never sent and is removed when hidden later,
//! and the props are sent when shown again
//! (`the_channel_never_sends_a_frozen_prop_the_clients_scope_does_not_take`, and in `frozen::server_tests`
//! `a_prop_out_of_a_clients_scope_is_never_sent_one_that_leaves_is_removed_and_one_that_enters_is_sent` and
//! `a_prop_another_plugin_keeps_from_a_client_is_never_sent_to_it_and_follows_the_plugins_hides`).
//!
//! **Extension points.** `engine.net().scope_control()` and `engine.net().stills()` (the engine seams the channel stands on,
//! not about props), `engine.net().ghost_share` (the props' share of each packet and their [`server::priority`]); the
//! collision sources ([`server::ServerSource`], [`client::ClientSource`]) other movers meet props through;
//! [`ServerProps::check_placement`] (`propCheckPlacement`) and `jointCheck`, which a game's own rate-limited, permission-checked
//! server commands call before `propSpawn` and the `joint*` functions; the tether sources ([`torque_game::player_tether`]) a
//! grapple is given to the Player through (and its predicted pulls reported back); [`JointBook::connected`], the seam the
//! prediction pulls a prop's jointed props in through; the collision sources' `object_of` and `hit` (torque-collide), which a
//! projectile's hit reaches the props through; the prediction slot's parts (torque-client), which [`predict::PropPart`] is;
//! and [`predict::PropPrediction`], a game's predicted impulses, blasts and spawns.
//!
//! **Checks.** `cargo test -p torque-plugin-props` and `cargo test -p torque-plugin-props --features rope --test joint_rope`;
//! the fuzzers with `TORQUE_FUZZ_PROPS=100000`; `just trace-hash` (the `props-drop`, `props-joints`, `carry`,
//! `projectile-props` and `hierarchy-props` lines must be equal native and wasm32, the other lines unchanged); `cargo bench -p torque-plugin-props
//! --bench step` and the `replay-bench` of torque-trace-hash (native and wasm32) under the bench lock.

pub mod client;
pub mod data;
pub mod frozen;
pub mod hierarchy;
pub mod hits;
pub mod hulls;
pub mod joints;
pub mod natives;
pub mod predict;
pub mod quant;
pub mod records;
pub mod server;
pub mod tick;
pub mod world;

use std::rc::Rc;

use torque_collide::source::PluginCollision;
#[cfg(feature = "rope")]
use torque_engine::Requires;
use torque_engine::{
    ClientStage, ClientTick, EngineBuilder, Plugin, PluginId, Replicate, ServerStage, ServerTick,
};

pub use client::{ClientProps, PropBook};
pub use data::{PropData, PropDataRecord, PropParams};
pub use joints::client::{ClientJoints, JointBook};
pub use joints::records::JointGhost;
pub use records::PropGhost;
pub use server::{PropPrefs, ServerProps};
pub use world::{Link, PropLimits, PropWorld};

/// The network version of the props plugin: a client and a server must run the same.
pub const NET_VERSION: u32 = 5;

/// The name of the ghost class a prop is.
pub const GHOST_CLASS: &str = "Prop";

/// The name of the props' collision source: their type bit is `$TypeMasks::PropObjectType`.
pub const COLLISION_NAME: &str = "Prop";

/// Adds props to the engine's servers and clients ([crate docs](crate)). Not in `DefaultPlugins`.
#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "torque.props")]
pub struct PropsPlugin;

// ANCHOR: build
impl Plugin for PropsPlugin {
    fn net_version(&self) -> Option<u32> {
        Some(NET_VERSION)
    }

    fn build(&self, engine: &mut EngineBuilder) {
        let server = ServerProps::default();
        let book = ClientProps::default();
        let joints = ClientJoints::default();
        engine.insert_resource(server.clone());
        engine.insert_resource(book.clone());
        engine.insert_resource(joints.clone());
        engine
            .net()
            .ghost::<JointGhost>(joints::GHOST_CLASS, Replicate::Interpolated);
        let heard_joints = joints.clone();
        engine
            .net()
            .on_ghost::<JointGhost>(joints::GHOST_CLASS, move |_, event| {
                joints::client::heard(&heard_joints, event)
            });
        // The grapples' tethers on players: the server's step asks the world, a client's prediction its books.
        let tethers = server.clone();
        engine.server(ServerStage::Classes, move |vm, _| {
            torque_game::sim::projectile::watch_explosions(
                vm,
                Rc::new(server::Blasts(tethers.clone())),
            );
            torque_game::player_tether::add(
                vm,
                torque_game::player_tether::Side::Server,
                joints::server::tethers(&tethers),
            );
            torque_game::scene::links::add_child_rule(
                vm,
                Rc::new(hierarchy::PropRule(tethers.clone())),
            );
            Ok(())
        });
        let client_tethers = joints::client::ClientTethers {
            joints: joints.clone(),
            props: book.clone(),
        };
        engine.client(ClientStage::Classes, move |vm, _| {
            torque_game::player_tether::add(
                vm,
                torque_game::player_tether::Side::Client,
                Rc::new(client_tethers.clone()),
            );
            Ok(())
        });
        engine
            .net()
            .ghost::<PropGhost>(GHOST_CLASS, Replicate::Interpolated)
            .datablock::<PropDataRecord>(<PropData as torque_engine::ScriptClass>::NAME);
        let (share, order) = (server.clone(), server.clone());
        engine.net().ghost_share(
            GHOST_CLASS,
            move |_| {
                share
                    .with(|s| s.net_share)
                    .unwrap_or(server::DEFAULT_NET_SHARE)
            },
            move |vm, obj, scope, skip| server::priority(&order, vm, obj, scope, skip),
        );
        engine
            .net()
            .drawn_shape::<PropGhost>(GHOST_CLASS, client::draw);
        let heard = book.clone();
        engine
            .net()
            .on_ghost::<PropGhost>(GHOST_CLASS, move |vm, event| {
                client::heard(vm, &heard, event)
            });
        // Where each prop is in the scene hierarchy, found after the client's other work each tick, and the frozen props sent
        // over the channel made ready to draw.
        let placed = book.clone();
        engine.push(ClientTick::new(move |vm| {
            client::refresh_places(vm, &placed)
        }));
        let edited = book.clone();
        engine.push(ClientTick::new(move |vm| {
            client::refresh_datablocks(vm, &edited)
        }));
        let published = (book.clone(), std::cell::Cell::new(0));
        engine.push(ClientTick::new(move |vm| {
            frozen::client::publish(vm, &published.0, &published.1)
        }));
        let frozen_book = book.clone();
        engine.net().scope_control().stills().channel_with(
            frozen::CHANNEL,
            frozen::limits(),
            move || frozen::client::FrozenChannel::new(frozen_book.clone()),
        );
        engine.push(PluginCollision::server(
            COLLISION_NAME,
            Rc::new(server::ServerSource(server.clone())),
        ));
        engine.push(PluginCollision::client(
            COLLISION_NAME,
            Rc::new(client::ClientSource {
                book: book.clone(),
                joints: joints.clone(),
            }),
        ));
        // Each server starts its props over; each client its book.
        let fresh = server.clone();
        engine.server(ServerStage::Classes, move |_, _| {
            fresh.reset();
            Ok(())
        });
        let (fresh, fresh_joints) = (book, joints);
        engine.client(ClientStage::Classes, move |_, _| {
            fresh.0.borrow_mut().clear();
            fresh_joints.0.borrow_mut().clear();
            Ok(())
        });
        engine
            .server_script()
            .class::<PropData>()
            .class::<server::NetProp>()
            .class::<joints::server::NetJoint>()
            .natives(natives::declared())
            .natives(joints::natives::declared())
            // Its prefs are the props' and joints' limits.
            .with_effects(torque_engine::Effects::ENGINE, |r| {
                r.prefs::<PropPrefs>();
            });
        engine
            .client_script()
            .class::<PropData>()
            .class::<server::NetProp>()
            .class::<joints::server::NetJoint>();
        engine.push(ServerTick::after(tick::hook(server)));
    }
}
// ANCHOR_END: build

/// `jointRope` on the server and the joints' rope driver on each client (the `rope` feature), for a game that adds it
/// beside [`PropsPlugin`] (required: added when missing) and the rope plugin. Cargo unifies features over a build, so what
/// a game's scripts can call comes with a plugin the game adds, never with the feature alone: a game without this plugin has
/// the same natives in every build.
#[cfg(feature = "rope")]
#[derive(Clone, Copy, Debug, Default, PluginId)]
#[plugin(id = "torque.props.rope")]
pub struct JointRopePlugin;

#[cfg(feature = "rope")]
impl Plugin for JointRopePlugin {
    fn requires(&self, requires: &mut Requires) {
        requires.plugin::<PropsPlugin>();
    }

    fn build(&self, engine: &mut EngineBuilder) {
        let joints = engine
            .resource::<ClientJoints>()
            .cloned()
            .expect("the props plugin, built first, inserted its joints");
        joints::rope::register(engine, &joints);
    }
}
