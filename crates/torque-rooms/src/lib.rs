//! A room service for player-hosted games: a player who hosts opens a room and is given its code, the players who join
//! name the code, and the service carries the signaling of one WebRTC data channel between each joiner and the host. The
//! game's datagrams go straight between them; nothing of the game passes through the service, and it keeps nothing but
//! the live rooms. It also gives the room's players their ICE servers: STUN, and TURN with short-lived credentials
//! ([`ice`]).
//!
//! [`rooms`] is the state machine (no I/O; the protocol is torque-server's `net::room`), and [`service`] runs it over
//! torque-server's web transport, whose HTTP and WebSocket handling, TLS, access checks and caps it takes as they are.

//!
//! # Maintaining
//!
//! **A room code cannot be guessed, so no limit refuses every network.** A code is 80 bits
//! (`torque_server::net::room::CODE_BITS`; `a_code_holds_at_least_64_bits` in torque-server): over at most 1,024 live
//! rooms a guess hits about once in 2^70 tries. A code shorter than 64 bits would need a stop that refuses everyone, which
//! a distributed flood could then hold.
//!
//! **A join's reply never tells a hit from a miss past a budget, and only the network over it is refused.** Junk is
//! refused before any count; every attempt counts for its network; every miss counts for its network and the total; past
//! its network's share every join from that network is `busy` until the window passes, hits too; the total only sheds
//! load, holding a network that missed already in the second to its share (`rooms::tests::
//! misses_charge_their_network_and_past_its_share_no_reply_tells_a_hit_from_a_miss`,
//! `a_flood_of_misses_from_many_networks_never_stops_a_joiner_from_a_quiet_network`,
//! `every_attempt_of_a_network_counts_and_over_it_the_reply_is_busy`, `junk_is_refused_before_anything_is_counted`). A new
//! refusal on the join path is checked before the room is looked up, or answers a miss as any miss.
//!
//! **Limits count per network**: rooms, new rooms, join attempts and misses key by an IPv4 /24 or an IPv6 /48
//! (`one_network_hosts_at_most_its_rooms`, `ipv6_networks_are_counted_by_their_48`).
//!
//! **A TURN user name is the player's address alone**, the service's own text, so every credential of a player is one user and
//! a TURN server's per-user quota is real; the TURN settings deny every range a relay could reach the TURN host's network,
//! the host itself or an IPv4 address through (`ice::tests::
//! a_user_name_is_the_services_own_text_and_names_the_player`, `the_turn_settings_deny_private_ranges_and_set_quotas`).

pub mod ice;
pub mod rooms;
pub mod service;

pub use ice::Ice;
pub use rooms::{RoomCounters, RoomLimits, Rooms};
pub use service::{Service, service_limits};
