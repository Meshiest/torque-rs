//! A game hosted by a browser (the `p2p` feature, built for the web): its server, run in the page's Web Worker (a
//! background tab slows the page's timers, not a worker's), on a relay transport (torque-server's `net::relay`) whose
//! links the page holds: the host player's own client (link 0, the page's main thread) and each joiner's data channel.
//! The page (`web/host.js`) tells the worker (`web/host-worker.js`) what each link does, and the worker calls these
//! ([`web_host_exports!`](crate::web_host_exports) gives a game's program the functions it calls); every cap on what
//! joiners send is the relay transport's.

use std::cell::RefCell;
use std::net::{IpAddr, Ipv4Addr};
use std::rc::Rc;

use torque_server::net::relay::{RelayLimits, RelayTransport};
use torque_server::net::transport::{SharedTransport, Transport};
use torque_server::server::TICK_MS;
use web_time::{Duration, Instant};

use crate::server::GameServer;

#[doc(hidden)]
pub use js_sys;

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

/// Starts the worker's server: `start` starts the game's server on the relay transport it is given (bound, with the
/// relay's default limits).
pub fn start(
    start: impl FnOnce(SharedTransport) -> Result<GameServer, String>,
) -> Result<(), String> {
    let relay = Rc::new(RefCell::new(RelayTransport::new(RelayLimits::default())));
    relay
        .borrow_mut()
        .bind(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
        .map_err(|e| e.to_string())?;
    let shared: SharedTransport = relay.clone();
    let server = start(shared)?;
    HOST.set(Some(Hosted {
        server,
        relay,
        next: Instant::now(),
    }));
    Ok(())
}

fn with_relay<T>(f: impl FnOnce(&mut RelayTransport) -> T) -> Option<T> {
    HOST.with_borrow(|h| h.as_ref().map(|h| f(&mut h.relay.borrow_mut())))
}

/// Link `id` opened from `ip` (the room service's word; the host player's own link is 0 from the loopback address).
/// False: refused by a cap (the page closes it).
pub fn open(id: u32, ip: &str) -> bool {
    let Ok(ip) = ip.parse::<Ipv4Addr>() else {
        return false;
    };
    with_relay(|r| r.open(id, IpAddr::V4(ip), Instant::now())).unwrap_or(false)
}

/// A message on link `id`. False: the link broke a cap and is closed (the page closes its channel).
pub fn datagram(id: u32, bytes: &[u8]) -> bool {
    with_relay(|r| r.datagram(id, bytes, Instant::now())).unwrap_or(false)
}

/// Link `id` closed.
pub fn close(id: u32) {
    with_relay(|r| r.close(id));
}

/// Runs the ticks that are due (32 ms each, at most a few to catch up), and returns what goes out: each link's number
/// followed by the datagram, in turn.
pub fn tick() -> js_sys::Array {
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
pub fn closed() -> Vec<u32> {
    with_relay(|r| r.take_closed().into_iter().map(|(id, _)| id).collect()).unwrap_or_default()
}

/// The functions a browser host's page and worker call (`web/host.js`, `web/host-worker.js`), in the game's program beside
/// [`web_exports!`](crate::web_exports): the host player's client (`startHostClient`, from `start_host_client`, a
/// function returning `Result<(), String>` that starts the client on [`crate::web_join::Target::Local`]) and its local
/// link (`localDatagram`, `setLocalSender`: [`crate::web_join`]), and the worker's server (`hostStart`, from
/// `host_start`, a function of the map asked for (empty: the game's first) returning `Result<String, String>`, the map
/// started, which calls [`start`]; `hostOpen`, `hostDatagram`, `hostClose`, `hostTick` and `hostClosed`: this module's).
/// The program depends on `wasm-bindgen`.
///
/// ```ignore
/// #[cfg(target_arch = "wasm32")]
/// torque::app::web_host_exports! {
///     start_host_client: my_game::web::start_host_client,
///     host_start: my_game::web::host_start,
/// }
/// ```
#[macro_export]
macro_rules! web_host_exports {
    (start_host_client: $start_host_client:path, host_start: $host_start:path $(,)?) => {
        mod host_page {
            use wasm_bindgen::prelude::wasm_bindgen;

            /// The host player's client, on the local link to the server in the page's worker.
            #[wasm_bindgen(js_name = startHostClient)]
            pub fn start_host_client() -> Result<(), String> {
                $start_host_client()
            }

            /// A datagram from the worker's server for the host player's client.
            #[wasm_bindgen(js_name = localDatagram)]
            pub fn local_datagram(bytes: Vec<u8>) {
                $crate::web_join::local_datagram(bytes)
            }

            /// The page's function that posts the host player's datagrams to the worker.
            #[wasm_bindgen(js_name = setLocalSender)]
            pub fn set_local_sender(sender: $crate::web_host::js_sys::Function) {
                $crate::web_join::set_local_sender(sender)
            }

            /// The worker's server: started on the fetched files and `map` (empty: the first); the map started.
            #[wasm_bindgen(js_name = hostStart)]
            pub fn host_start(map: &str) -> Result<String, String> {
                $host_start(map)
            }

            #[wasm_bindgen(js_name = hostOpen)]
            pub fn host_open(id: u32, ip: &str) -> bool {
                $crate::web_host::open(id, ip)
            }

            #[wasm_bindgen(js_name = hostDatagram)]
            pub fn host_datagram(id: u32, bytes: &[u8]) -> bool {
                $crate::web_host::datagram(id, bytes)
            }

            #[wasm_bindgen(js_name = hostClose)]
            pub fn host_close(id: u32) {
                $crate::web_host::close(id)
            }

            #[wasm_bindgen(js_name = hostTick)]
            pub fn host_tick() -> $crate::web_host::js_sys::Array {
                $crate::web_host::tick()
            }

            #[wasm_bindgen(js_name = hostClosed)]
            pub fn host_closed() -> Vec<u32> {
                $crate::web_host::closed()
            }
        }
    };
}
