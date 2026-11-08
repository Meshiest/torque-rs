//! The network host: the datagram transport under it ([`transport`]), the seam between the packet layer and what rides in the
//! packets ([`payload`]), the codecs a game's protocol sets ([`codecs`]), the frame of the datablock events ([`datablock`]),
//! and the host itself over a game's parts ([`host`]: [`NetShared`], [`NetGame`]): the passes ([`pump`]), the connections
//! ([`conn`], [`local`]), the ghosting natives and the `NetObject` behaviour ([`ghosting`]), the event layer ([`events`],
//! [`hostevent`]), the bulk channels ([`channels`]) and the content a server sends before a client's game starts ([`content`]), the datablock chain ([`datablocks`]), the ghost hub ([`ghosts`]), the command natives ([`commands`]) and the server query's answers ([`query`]); [`torque`] is Torque's
//! own net game and the network host the main loop drives. With the `web` feature, [`web`] is the transport browsers join
//! over (WebSocket, with the game's page served on the same port) and the one that takes UDP and the web together. With the
//! `rooms` feature, [`room`] is the protocol of a room service, by which a player who hosts a game and the players who
//! join it find each other.

pub mod channels;
pub mod cmdlimit;
pub mod codecs;
pub mod commands;
pub mod conn;
pub mod content;
pub mod datablock;
pub mod datablocks;
pub mod events;
pub mod ghosting;
pub mod ghosts;
pub mod host;
pub mod hostevent;
pub mod local;
pub mod payload;
pub mod peer;
pub mod pump;
pub mod query;
#[cfg(feature = "p2p")]
pub mod relay;
#[cfg(feature = "rooms")]
pub mod room;
pub mod torque;
pub mod transport;
#[cfg(all(feature = "web", not(target_arch = "wasm32")))]
pub mod web;

pub use host::*;
