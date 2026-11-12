//! The dedicated server's engine side: [`game`] (TGE's game classes put together over one container and the process list),
//! [`platform`] (the machine and the network a host's natives report), [`pacing`] (the
//! wall-clock pacing of a realtime main loop), [`net`] (the datagram transport and its bind policy, and the payload seam the
//! packet layer hands every data packet to) and [`state`] (the engine state the natives and the network host share).
//!
//! # Maintaining
//!
//! **A datablock record's extension blocks follow the peer.** `ConnDatablocks::set_extensions` is true for a peer of
//! `PeerKind::Engine` only (every peer of a server that speaks the engine's protocol), so `NetCodecs::datablock_record` packs
//! `ParticleData` and `ParticleEmitterData` with the engine's block after TGE's bits for that peer alone, and Torque's protocol
//! stays bit for bit TGE's (torque-trace-hash's `wire_golden`; torque-ghosts' `torques_records_end_where_tges_do_and_the_engines_carry_the_extension_after`).
//!
//! A limit a pref sets is held under a cap of the engine's own, whoever wrote the pref: `$Pref::Server::MaxPlayers` never
//! lets in more than `net::pump::MAX_PLAYERS` (`max_players_is_held_under_the_engines_cap`).
//!
//! **A connection's identity is read from the handshake table, never from the client**: `GameConnection::getIdentity` and
//! `getIdentityName` (`net/permissions.rs`) answer from the identity the
//! connect gate verified (`a_signed_in_player_is_the_same_identity_on_every_connection_whatever_the_client_claims`,
//! `an_anonymous_connection_is_its_own_player_and_no_ticket_makes_it_anyone_else` in `tests/server.rs`). A script that must not
//! lose a player to a reconnect keys by it; the natives are registered with the permissions natives. A plugin never reads it
//! from script (a game may redefine the method): `torque_engine::permissions::ConnectionSubjects`, attached beside the natives,
//! answers from the table. An account that connects again ends its older connections before the new `onConnect`
//! (`$Pref::Server::OneSessionPerAccount`, on unless set to 0; `pump::end_older_sessions`).
//!
//! **Every per-address and per-network count keys by the peer's own address** (the transport's `peer_ip`, a web peer's real
//! one behind its stand-in): the handshake's caps, the request and reject budgets, the download places, the info limits and
//! the strangers' shares (`peers_behind_stand_ins_from_one_real_network_share_its_caps` in `tests/server.rs`).
//!
//! **Strangers cannot starve players**: a datagram that is not a connection's game packet draws on the strangers' pool and
//! past it is read and dropped at no cost, without a copy, up to `NET_READS_PER_PASS` a pass
//! (`junk_from_many_sources_never_starves_a_joined_client`,
//! `a_flood_past_a_passs_old_read_cap_still_lets_a_players_packets_through_in_a_tick_or_two`,
//! `game_flagged_datagrams_from_no_connection_share_their_ips_work_whatever_the_port`).
//!
//! **The ghost pass reads each object once and sorts nothing a packet**: a registry entry carries its class code and whether the
//! class is the engine's own, `TgeSource` finds a class, a share and a peer's sight from it (no name is compared per object), the
//! ids of a walk are kept sorted per generation of the registry (`Entries::generation`, which every add, remove and edit of an
//! entry moves, and with it the source's `scope_stamp`), and the controlled objects are searched in order. Ghosts of objects
//! added or removed after a pass settled still reach every connection, and a settled pass spends the scan units of an unsettled
//! one (`tests/ghost_pass.rs`); the bench is `ghost_tick` (64 connections, 1,024 objects: static, 100 and all marked).
//!
//! **The per-ghost block never changes a TGE record** ([`net::blocks`]): it follows a record only for a peer of the engine's own
//! on a server that installed a writer (the hierarchy plugin, in the engine's protocol), the class record never sees the
//! block's mask bits (`torque_ghosts::block::BLOCK_MASK`) and never keeps them; without a writer `TgeSource` packs, scopes, ranks
//! and stamps exactly as before (the `tge` lines of `just trace-hash`, the wire goldens). With one, its links' revision moves
//! the source's `scope_stamp`, so a settled query never misses a relative that was just linked
//! (torque-test-app's `hierarchy_net`).
//!
//! **The plugins' net classes go only where a net game carries them**: `TgeGhostGame` answers
//! `GhostGame::carries_plugin_classes` with true, every other ghost game false unless it says so (one that wraps
//! `TgeGhostGame` and delegates its sources does). torque-app-server warns at a server's start when the engine has plugin
//! classes and the ghost game or the codecs do not carry them (`net_game_codecs_that_do_not_carry_the_plugin_classes_are_warned_about`
//! in torque-trace-hash's `tests/wire_golden.rs`); a peer that is not the engine's own never sees them either way.
//!
//! **A command in the engine's protocol costs what a packet's costs** (`net::command_rpc`): the RPC's bucket bounds what a body costs to
//! read, a body that is not one ends the connection, every command goes through the same gates and limits as a packet's
//! (`run_followups`), and a command no plugin declared shares the default limits
//! (`commands_modes::a_command_no_plugin_declared_is_held_to_the_default_limits_in_the_engines_protocol_only`).
//!
//! **A flood of info queries runs no script**: the info answer is made from the last second's script output
//! (`a_flood_of_info_queries_runs_the_info_script_once_a_second_and_every_one_is_answered`).
//!
//! **Live datablocks drain once a tick, before the pass** (`server.rs`, the tick branch of `run_ticks_on`:
//! `torque_game::scene::refresh::drain`, its errors kept with the tick's), after the packets' scripts; `game.rs` installs the set
//! with the game classes and moves a datablock's user count in `GameBase::onAdd` and `onRemove`, and `net/datablocks.rs` chains
//! the key behavior onto `SimDataBlock` instead of replacing it, so the redeclare mark the game classes put there stays. A host
//! that replaces `GameBase`'s behavior keeps the counts or loses the refresh (`tests/datablock_reload.rs`).
//! Right before the drain the tick polls the shape files (`torque_game::shapes::reload::poll`, gated by
//! `torque_engine::script::dev_allowed` and paced by `torque_engine::assets::HOT_RELOAD_INTERVAL_MS` on the simulation clock):
//! a changed `.dts` marks its datablocks preloaded for that same drain. A release server with `$Pref::Dev` unset reads no file
//! time there (`tests/shape_reload.rs`).
//!
//! **A changed datablock file is reloaded through one path** (`reload.rs`): `reloadDataBlocks(%file [, %removeMissing])`
//! and the development watcher (`reload::poll`, run by `server.rs` before the tick's drain, only when
//! `torque_engine::script::dev_allowed_in` says so and once per `HOT_RELOAD_INTERVAL_MS` of server time) both run
//! `reload::reload`: `Vm::exec_file_checked` (the source, nothing of a source with a syntax error), the refresh drained, the
//! `before_round` hooks, one live round through the `LiveRound` the network host gives `reload::install`. Invariants, each
//! in `tests/datablock_watch.rs`: a changed file is reloaded within a poll interval and a tick and its values reach a
//! connected client; an unchanged file is never run again; a syntax error keeps every datablock, prints one error and is
//! retried only when the file changes again; outside development no change time is read and the native is refused; the
//! datablocks a file no longer declares are reported, and removed only with `%removeMissing` or `$Pref::Dev::RemoveMissing`
//! and only without users (a refusal is an error naming the count); a file that registers other objects is warned about
//! once. The removal of a missing datablock is the plain `delete()` behind a user-count check; the checked removal with a
//! fallback is the one to use once it exists. Extension point: `reload::before_round` (a plugin reads its content again
//! before the round; capped at `MAX_BEFORE_ROUND_HOOKS`). Check: `cargo test -p torque-server --test main datablock_watch`.
//!
//! **A datablock's removal is the game's reference-counted one** (`game.rs` registers `torque_game::scene::remove`, server
//! only): a retiring datablock is a live object, so a connection that joins still gets it in its chain and `net/datablocks.rs`
//! renders it as any other (its key, forgotten by `KeyedClass::on_remove` only when it is finally deleted); a removed one's
//! key is forgotten and the others' keys do not move (`a_client_that_joins_after_a_removal_gets_the_rest_with_their_keys`).
//! `net/torque.rs`'s `GameView::datablock` hands the codecs no id of a deleted datablock
//! (`a_datablock_deleted_under_its_players_leaves_them_their_info_and_no_dead_id_on_the_wire`). In Torque's protocol nothing
//! is posted to a connection on a removal (`tests/datablock_remove.rs`).
//!
//! **In the engine's protocol a deleted datablock is told to the clients that had it** (`net/host.rs`: a host attached under
//! `ProtocolMode::Engine` derives the engine's event table, `codecs::engine_event_registry`, and sets the game's removal hook to
//! `EventLayer::remove_datablock`): the hook runs at the deletion, never at the retire (a retiring datablock still travels to a
//! joining client and a user's ghost can name it again), and `ConnDatablocks::remove` posts a `DataBlockRemoveEvent` through the
//! chain's hold queue (`post_or_hold`), on the ordered queue behind the body of the same id, to the connections whose `sent`
//! table holds the id and to no other (`tests/datablock_remove_wire.rs`: an engine client over the memory link at 20% loss with
//! reordering, 100 seeded orders; a server of Torque's protocol posts nothing and keeps the 13-class table). A game that sets a
//! removal hook of its own after the host is attached must post the event itself.
//!
//! **Nothing a client sends runs free**: a remote command that names no function is charged to its connection's share, and
//! its warning is the script engine's, limited per connection (`Vm::invoke_from` with the connection as its source; one
//! limiter, not a second here) (`unknown_commands_cost_the_connections_share_and_their_warnings_are_limited` in
//! torque-app's `tests/server_commands.rs`); a held download progresses only at `ContentLimits::min_bytes_per_s`, counting the bytes of the runs it asked for that the link acknowledged
//! (`progress_is_bytes_served_at_the_least_rate`). The whole receive path is fuzzed with a real client's mutated packets
//! (`hostile_packets_of_an_established_connection_never_panic_the_server`, `TORQUE_FUZZ_SERVER`).
//!
//! **A link's facts and binding come from the transport, per peer** (`Transport::link_kind`, `Transport::peer_ip`,
//! `Transport::channel_binding`): engine mode reads them when a datagram arrives and when it judges a connect, so a transport
//! that names its peers by stand-in addresses (the web transport, the QUIC link in `torque-net-quic`) counts and binds them
//! right. The engine's ticket context (`net::engine::Checks::context`) takes the peer's link kind (its server identity) and
//! the transport's channel binding; a link without them (UDP, memory, web) gives the context the same bytes as before
//! (torque-net's `bridge::tests::the_ticket_context_differs_per_challenge_server_identity_and_binding`; torque-app's
//! `tests/quic_link.rs`, `a_ticket_is_bound_to_its_connections_channel_binding`).
//! **Datablocks change on the wire only when a round is asked for** (`net::datablocks`): `updateDataBlocks()` starts a live
//! round on every connection past its mission chain, which sends the networked datablocks whose `modifiedKey` differs from the
//! one that connection was last sent a body with (`ConnDatablocks::sent_key`, filled by every render, the mission chain's
//! too), the ones it never had first and then the changed ones by key, so a referrer follows a new datablock it names, as the same events with ordinals for index and
//! total; a connection still in its mission chain, or with a chain running, gets the round after it (coalesced). Its done
//! message carries a negative sequence of its own, and the client's answer runs `onDataBlocksUpdated(count)`, never
//! `onDataBlocksDone` again; a round with nothing to send sends nothing. With no round asked the wire is Torque's
//! (torque-trace-hash's `wire_golden` in both protocols); `tests/datablock_live.rs` has the chain (one body per edit, the
//! order, an edit during a download, loss, the callbacks) and torque-test-app's `server_datablock_live` whole clients in
//! both protocols with their caps. In the engine's protocol a connection's control object is held back from a change of its
//! datablock until the client has read the record (`net::host`'s `PredictedHold`, `ConnDatablocks::awaits`), so its moves are
//! stepped with the values the client predicts them with: 0 corrections from the change on a 20% loss link
//! (torque-plugin-props's `datablock_prediction`). Torque's protocol applies it at once, as Torque does.
//!
//! **A live round waits for a connection's content** (`net::content`, `net::datablocks`): the game's `ContentServer` names
//! the connections it sent content to mid-game (`take_late`), and the host holds each as it held it at the accept (a late
//! `Awaiting`: the burst, the same idle and total limits, dropped with the reason when it stalls, every command still taken,
//! no second `onConnect`) and sets `ConnDatablocks::hold_for_content`, which makes a round asked for (or one waiting for a
//! chain's end) wait until `content_ready` says the connection has what it needs; `poll_datablock_holds` releases it once a
//! tick. So a datablock whose files the client lacks is never sent, and a client that never says ready is dropped by the same
//! timeouts a joining one is. torque-mods' `tests/late_offer.rs` runs it over the memory transport with loss.

pub mod game;
pub mod net;
pub mod pacing;
pub mod platform;
pub mod reload;
pub mod server;
pub mod state;
