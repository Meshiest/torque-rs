//! The network host: the datagram transport under it ([`transport`]), the seam between the packet layer and what rides in the
//! packets ([`payload`]), the codecs a game's protocol sets ([`codecs`]), the frame of the datablock events ([`datablock`]),
//! and the host itself over a game's parts ([`host`]: [`NetShared`], [`NetGame`]): the passes ([`pump`]), the connections
//! ([`conn`], [`local`]), the event layer ([`events`], [`hostevent`]), the datablock chain ([`datablocks`]), the ghost hub
//! ([`ghosts`]), the command natives ([`commands`]) and the server query's answers ([`query`]); [`torque`] is Torque's
//! own net game and the network host the main loop drives.

pub mod codecs;
pub mod commands;
pub mod conn;
pub mod datablock;
pub mod datablocks;
pub mod events;
pub mod ghosts;
pub mod host;
pub mod hostevent;
pub mod local;
pub mod payload;
pub mod pump;
pub mod query;
pub mod torque;
pub mod transport;

pub use host::*;
