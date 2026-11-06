//! A room service for player-hosted games: a player who hosts opens a room and is given its code, the players who join
//! name the code, and the service carries the signaling of one WebRTC data channel between each joiner and the host. The
//! game's datagrams go straight between them; nothing of the game passes through the service, and it keeps nothing but
//! the live rooms. It also gives the room's players their ICE servers: STUN, and TURN with short-lived credentials
//! ([`ice`]).
//!
//! [`rooms`] is the state machine (no I/O; the protocol is torque-server's `net::room`), and [`service`] runs it over
//! torque-server's web transport, whose HTTP and WebSocket handling, TLS, access checks and caps it takes as they are.

pub mod ice;
pub mod rooms;
pub mod service;

pub use ice::Ice;
pub use rooms::{RoomCounters, RoomLimits, Rooms};
pub use service::{Service, service_limits};
