//! The arena hosted by a browser: its server, run in the page's Web Worker on the engine's relay transport
//! ([`torque::app::web_host`], which holds every cap on what joiners send); the page (`web/host.js`) and the worker
//! (`web/host-worker.js`) call these.

use std::net::Ipv4Addr;

use crate::server::{ArenaServer, ServerOptions};

pub use torque::app::web_host::{
    close as host_close, closed as host_closed, datagram as host_datagram, open as host_open,
    tick as host_tick,
};

/// Starts the server on the fetched files and `map` (empty: the first), and returns the map's path.
pub fn host_start(map: &str) -> Result<String, String> {
    let files = crate::web::fetched_files()?;
    let mission = if map.is_empty() {
        files
            .maps()
            .into_iter()
            .next()
            .ok_or("no map in the game folder's missions")?
    } else {
        map.to_string()
    };
    let options = ServerOptions {
        address: Ipv4Addr::LOCALHOST,
        port: 0,
        name: "Arena".to_string(),
        mission: mission.clone(),
        public: false,
    };
    torque::app::web_host::start(|relay| ArenaServer::start_bound(&files, &options, relay))?;
    Ok(mission)
}
