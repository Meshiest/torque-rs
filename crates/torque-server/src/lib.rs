//! The dedicated server's engine side: [`game`] (TGE's game classes put together over one container and the process list),
//! [`platform`] (the machine and the network a host's natives report), [`pacing`] (the
//! wall-clock pacing of a realtime main loop), [`net`] (the datagram transport and its bind policy, and the payload seam the
//! packet layer hands every data packet to) and [`state`] (the engine state the natives and the network host share).
//!
//! # Maintaining
//!
//! A limit a pref sets is held under a cap of the engine's own, whoever wrote the pref: `$Pref::Server::MaxPlayers` never
//! lets in more than `net::pump::MAX_PLAYERS` (`max_players_is_held_under_the_engines_cap`).

pub mod game;
pub mod net;
pub mod pacing;
pub mod platform;
pub mod server;
pub mod state;
