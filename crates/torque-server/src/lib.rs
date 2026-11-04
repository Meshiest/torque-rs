//! The dedicated server's engine side: [`platform`] (the machine and the network a host's natives report), [`pacing`] (the
//! wall-clock pacing of a realtime main loop), [`net`] (the datagram transport and its bind policy, and the payload seam the
//! packet layer hands every data packet to) and [`state`] (the engine state the natives and the network host share).

pub mod net;
pub mod pacing;
pub mod platform;
pub mod server;
pub mod state;
