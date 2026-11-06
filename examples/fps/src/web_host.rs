//! The arena hosted by a browser: its server, run in the page's Web Worker (a background tab slows the page's timers, not a
//! worker's), on a relay transport (torque-server's `net::relay`) whose links the page holds: the host player's own client
//! (link 0, the page's main thread) and each joiner's data channel. The page (`web/host.js`) tells the worker
//! (`web/host-worker.js`) what each link does, and the worker calls these; every cap on what joiners send is the relay
//! transport's.

use std::cell::RefCell;
use std::net::{IpAddr, Ipv4Addr};
use std::rc::Rc;

use torque::app::server::GameServer;
use torque::server::net::relay::{RelayLimits, RelayTransport};
use torque::server::net::transport::{SharedTransport, Transport};
use torque::server::server::TICK_MS;
use web_time::{Duration, Instant};

use crate::server::{ArenaServer, ServerOptions};

/// The most ticks one call runs when the worker fell behind (it then catches up from now).
const MAX_CATCH_UP: u32 = 4;

struct Hosted {
    server: GameServer,
    relay: Rc<RefCell<RelayTransport>>,
    next: Instant,
}

thread_local! {
    static HOST: RefCell<Option<Hosted>> = const { RefCell::new(None) };
}

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
    let relay = Rc::new(RefCell::new(RelayTransport::new(RelayLimits::default())));
    relay
        .borrow_mut()
        .bind(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
        .map_err(|e| e.to_string())?;
    let shared: SharedTransport = relay.clone();
    let server = ArenaServer::start_bound(&files, &options, shared)?;
    HOST.set(Some(Hosted {
        server,
        relay,
        next: Instant::now(),
    }));
    Ok(mission)
}

fn with_relay<T>(f: impl FnOnce(&mut RelayTransport) -> T) -> Option<T> {
    HOST.with_borrow(|h| h.as_ref().map(|h| f(&mut h.relay.borrow_mut())))
}

/// Link `id` opened from `ip` (the room service's word; the host player's own link is 0 from the loopback address).
/// False: refused by a cap (the page closes it).
pub fn host_open(id: u32, ip: &str) -> bool {
    let Ok(ip) = ip.parse::<Ipv4Addr>() else {
        return false;
    };
    with_relay(|r| r.open(id, IpAddr::V4(ip), Instant::now())).unwrap_or(false)
}

/// A message on link `id`. False: the link broke a cap and is closed (the page closes its channel).
pub fn host_datagram(id: u32, bytes: &[u8]) -> bool {
    with_relay(|r| r.datagram(id, bytes, Instant::now())).unwrap_or(false)
}

/// Link `id` closed.
pub fn host_close(id: u32) {
    with_relay(|r| r.close(id));
}

/// Runs the ticks that are due (32 ms each, at most a few to catch up), and returns what goes out: each link's number
/// followed by the datagram, in turn.
pub fn host_tick() -> js_sys::Array {
    let out = js_sys::Array::new();
    HOST.with_borrow_mut(|h| {
        let Some(h) = h.as_mut() else {
            return;
        };
        let tick = Duration::from_millis(u64::from(TICK_MS));
        let now = Instant::now();
        let mut ran = 0;
        while h.next <= now && ran < MAX_CATCH_UP {
            h.server.tick();
            // The console's lines are not shown on the page.
            h.server.drain_log();
            h.next += tick;
            ran += 1;
        }
        if h.next <= now {
            h.next = now + tick;
        }
        for (id, data) in h.relay.borrow_mut().take_out() {
            out.push(&id.into());
            out.push(&js_sys::Uint8Array::from(data.as_slice()));
        }
    });
    out
}

/// The links the relay closed for breaking a cap since the last call.
pub fn host_closed() -> Vec<u32> {
    with_relay(|r| r.take_closed().into_iter().map(|(id, _)| id).collect()).unwrap_or_default()
}
