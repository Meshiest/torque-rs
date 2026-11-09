//! The dedicated server's engine side: [`game`] (TGE's game classes put together over one container and the process list),
//! [`platform`] (the machine and the network a host's natives report), [`pacing`] (the
//! wall-clock pacing of a realtime main loop), [`net`] (the datagram transport and its bind policy, and the payload seam the
//! packet layer hands every data packet to) and [`state`] (the engine state the natives and the network host share).
//!
//! # Maintaining
//!
//! A limit a pref sets is held under a cap of the engine's own, whoever wrote the pref: `$Pref::Server::MaxPlayers` never
//! lets in more than `net::pump::MAX_PLAYERS` (`max_players_is_held_under_the_engines_cap`).
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
//! **A flood of info queries runs no script**: the info answer is made from the last second's script output
//! (`a_flood_of_info_queries_runs_the_info_script_once_a_second_and_every_one_is_answered`).
//!
//! **Nothing a client sends runs free**: a remote command that names no function is charged to its connection's share, and
//! its warning is the script engine's, limited per connection (`Vm::invoke_from` with the connection as its source; one
//! limiter, not a second here) (`unknown_commands_cost_the_connections_share_and_their_warnings_are_limited` in
//! torque-app's `tests/server_commands.rs`); a held download progresses only at `ContentLimits::min_bytes_per_s`
//! (`progress_is_bytes_served_at_the_least_rate`). The whole receive path is fuzzed with a real client's mutated packets
//! (`hostile_packets_of_an_established_connection_never_panic_the_server`, `TORQUE_FUZZ_SERVER`).

pub mod game;
pub mod net;
pub mod pacing;
pub mod platform;
pub mod server;
pub mod state;
