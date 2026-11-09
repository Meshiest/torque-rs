//! Hosting a game and joining one, as a game's client does it: where its socket binds, which address a hosted game binds and
//! is joined by, and the address a player types.
//!
//! - [`NetMode`]: [`NetMode::Loopback`] binds the client's socket to `127.0.0.1` and reads only loopback datagrams (tests, a
//!   game for one); [`NetMode::Lan`] binds it as TGE binds one (every address, with a loopback twin so a server on `127.0.0.1`
//!   hears its LAN query) and talks to LAN addresses only, but for one public address the player typed ([`register_join_natives`]).
//! - A hosted game binds this computer's LAN address ([`lan_bind_address`], found from the system's routing, or the one the
//!   player named), and others on the network join it by one of [`lan_reach`]; the hosting client joins its own server by
//!   [`join_address`].
//! - [`read_join_address`] reads a typed address or host name; a name is looked up off the frame by [`JoinLookup`], and its
//!   address gets the checks typed numbers get. [`register_join_natives`] gives the scripts `checkJoinAddress`,
//!   `joinLookupResult`, `cancelJoinLookup` and `armPublicJoin`.

use std::cell::RefCell;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use web_time::{Duration, Instant};

use super::client::ClientNet;
use super::plugins::client_wiring;
use super::server::lan_addresses;
use torque_client::conn::tge::TgeConn;
use torque_client::state::ClientState;
use torque_engine::{ClientStage, EngineBuilder, Plugin, Requires};
use torque_ts_vm::vm::{Native, Vm};

/// Where a client's socket binds and what it reaches (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetMode {
    Loopback,
    Lan,
}

impl NetMode {
    /// The client host's network for this mode.
    pub fn client_net(self) -> ClientNet {
        match self {
            NetMode::Loopback => ClientNet::Loopback,
            NetMode::Lan => ClientNet::Lan,
        }
    }
}

/// The addresses other players on this computer's network join a hosted game by: `named` when it is not the loopback address,
/// else this computer's own LAN addresses; none in [`NetMode::Loopback`].
pub fn lan_reach(net: NetMode, named: Ipv4Addr) -> Vec<Ipv4Addr> {
    match net {
        NetMode::Loopback => Vec::new(),
        NetMode::Lan if named.is_loopback() => lan_addresses(),
        NetMode::Lan => vec![named],
    }
}

/// The address a LAN game binds: the first of [`lan_reach`], else `named` (loopback: only this computer can join).
pub fn lan_bind_address(net: NetMode, named: Ipv4Addr) -> Ipv4Addr {
    lan_reach(net, named).first().copied().unwrap_or(named)
}

/// The address the hosting client joins its own server by: the server's, or this computer's loopback address for a server
/// on every interface.
pub fn join_address(bound: SocketAddr) -> SocketAddr {
    let ip = if bound.ip().is_unspecified() {
        Ipv4Addr::LOCALHOST.into()
    } else {
        bound.ip()
    };
    SocketAddr::new(ip, bound.port())
}

/// What a typed address names: a numbered address, checked, or a host name still to be looked up (see [`JoinLookup`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinTarget {
    Address(SocketAddrV4),
    Name { name: String, port: u16 },
}

const NOT_AN_ADDRESS: &str = "That is not an address. Use numbers like 192.168.1.20:28000.";

/// Reads a typed address: `a.b.c.d`, `name` (port [`DEFAULT_PORT`](super::server::DEFAULT_PORT)) or either with `:port`, with
/// or without an `IP:` in front. Numbers get [`check_join_address`]; a name must be a valid host name (RFC 1123) and is not
/// looked up here.
pub fn read_join_address(text: &str) -> Result<JoinTarget, &'static str> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Type the game's address, like 192.168.1.20:28000.");
    }
    let text = text
        .strip_prefix("IP:")
        .or_else(|| text.strip_prefix("ip:"))
        .unwrap_or(text);
    let (host, port) = match text.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().map_err(|_| NOT_AN_ADDRESS)?),
        None => (text, super::server::DEFAULT_PORT),
    };
    if port == 0 {
        return Err("Ports are numbers from 1 to 65535.");
    }
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return check_join_address(SocketAddrV4::new(ip, port)).map(JoinTarget::Address);
    }
    if host.eq_ignore_ascii_case("broadcast") {
        return Err("That address cannot be a game.");
    }
    if host.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return Err(NOT_AN_ADDRESS);
    }
    if !is_host_name(host) {
        return Err(
            "That is not an address or a name. Use one like 192.168.1.20:28000 or games.example.com.",
        );
    }
    Ok(JoinTarget::Name {
        name: host.to_string(),
        port,
    })
}

/// Whether `name` is a host name: at most 253 characters (a trailing dot aside), labels of 1 to 63 letters, digits and
/// hyphens, none starting or ending with a hyphen.
fn is_host_name(name: &str) -> bool {
    let name = name.strip_suffix('.').unwrap_or(name);
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

/// `address` when it can be a game's: no port 0, nor this network (0/8), multicast, reserved (240/4) or a broadcast (a last
/// number of 255, as on a /24 LAN). A looked-up name's address gets the same checks.
pub fn check_join_address(address: SocketAddrV4) -> Result<SocketAddrV4, &'static str> {
    let o = address.ip().octets();
    if address.port() == 0 {
        Err("Ports are numbers from 1 to 65535.")
    } else if o[0] == 0 || o[0] >= 224 || o[3] == 255 {
        Err("That address cannot be a game.")
    } else {
        Ok(address)
    }
}

/// A typed address in numbers (see [`read_join_address`]); a name gets a message saying it must be looked up first.
pub fn parse_join_address(text: &str) -> Result<SocketAddrV4, &'static str> {
    match read_join_address(text)? {
        JoinTarget::Address(address) => Ok(address),
        JoinTarget::Name { .. } => Err("That name has to be looked up first."),
    }
}

/// Looks up a host name's addresses (`name`, `port`): the system's ([`system_resolver`]) or a test's.
pub type Resolver = Arc<dyn Fn(&str, u16) -> io::Result<Vec<SocketAddr>> + Send + Sync>;

/// The system's resolver (its hosts file, then DNS).
pub fn system_resolver() -> Resolver {
    Arc::new(|name, port| Ok((name, port).to_socket_addrs()?.collect()))
}

/// How long a name's lookup may take.
pub const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Why a name's lookup gives no address to join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupError {
    NotFound,
    TimedOut,
    NoIpv4,
    NotAllowed,
    /// [`MAX_LOOKUPS`] threads still wait on the system.
    Busy,
}

impl LookupError {
    /// The line the join screen shows.
    pub fn message(self) -> &'static str {
        match self {
            LookupError::NotFound => "That name was not found. Check the spelling.",
            LookupError::TimedOut => {
                "Looking up that name took too long. Try again, or type the address in numbers."
            }
            LookupError::NoIpv4 => {
                "That name has no address the game can use. Type the address in numbers."
            }
            LookupError::NotAllowed => "That name leads to an address that cannot be a game.",
            LookupError::Busy => "Still looking up the last name. Try again in a moment.",
        }
    }
}

/// The most lookup threads alive at once in the process: a canceled or timed-out lookup's thread waits on the system until it
/// answers (up to half a minute), so clicking again must not pile more up.
pub const MAX_LOOKUPS: usize = 2;

/// The lookup threads alive.
static LOOKUPS: AtomicUsize = AtomicUsize::new(0);

/// One of [`MAX_LOOKUPS`], given back when the thread holding it ends (or never starts).
struct Slot(&'static AtomicUsize);

impl Slot {
    fn take(count: &'static AtomicUsize) -> Option<Slot> {
        count
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_LOOKUPS).then_some(n + 1)
            })
            .ok()
            .map(|_| Slot(count))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A host name looked up on a thread of its own, so the frame never waits: [`JoinLookup::poll`] it each frame, or drop it to
/// cancel (the thread's answer is then thrown away).
pub struct JoinLookup {
    name: String,
    port: u16,
    answer: mpsc::Receiver<io::Result<Vec<SocketAddr>>>,
    deadline: Instant,
}

impl JoinLookup {
    /// Starts a lookup, or [`LookupError::Busy`] when [`MAX_LOOKUPS`] threads still run.
    pub fn start(
        name: &str,
        port: u16,
        resolver: Resolver,
        timeout: Duration,
    ) -> Result<Self, LookupError> {
        Self::start_in(&LOOKUPS, name, port, resolver, timeout)
    }

    fn start_in(
        slots: &'static AtomicUsize,
        name: &str,
        port: u16,
        resolver: Resolver,
        timeout: Duration,
    ) -> Result<Self, LookupError> {
        let slot = Slot::take(slots).ok_or(LookupError::Busy)?;
        let (tx, answer) = mpsc::channel();
        let owned = name.to_string();
        // A thread that cannot start drops `tx` and the slot: the lookup ends as not found.
        let _ = std::thread::Builder::new()
            .name("join-lookup".into())
            .spawn(move || {
                let answer = resolver(&owned, port);
                drop(slot);
                let _ = tx.send(answer);
            });
        Ok(JoinLookup {
            name: name.to_string(),
            port,
            answer,
            deadline: Instant::now() + timeout,
        })
    }

    /// Whether this looks up `target` (names compare without case).
    fn is_for(&self, target: &JoinTarget) -> bool {
        matches!(target, JoinTarget::Name { name, port }
            if name.eq_ignore_ascii_case(&self.name) && *port == self.port)
    }

    /// The name being looked up.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The address to join once the lookup ends (the first IPv4 answer, with [`check_join_address`]'s checks), or `None`
    /// while it runs.
    pub fn poll(&self) -> Option<Result<SocketAddrV4, LookupError>> {
        match self.answer.try_recv() {
            Ok(answer) => Some(self.finish(answer)),
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(LookupError::NotFound)),
            Err(mpsc::TryRecvError::Empty) => {
                (Instant::now() >= self.deadline).then_some(Err(LookupError::TimedOut))
            }
        }
    }

    /// Waits for the end of the lookup (a command line's join, not a frame's).
    pub fn wait(self) -> Result<SocketAddrV4, LookupError> {
        match self
            .answer
            .recv_timeout(self.deadline.saturating_duration_since(Instant::now()))
        {
            Ok(answer) => self.finish(answer),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(LookupError::NotFound),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(LookupError::TimedOut),
        }
    }

    fn finish(&self, answer: io::Result<Vec<SocketAddr>>) -> Result<SocketAddrV4, LookupError> {
        let addresses = answer.map_err(|_| LookupError::NotFound)?;
        if addresses.is_empty() {
            return Err(LookupError::NotFound);
        }
        let ip = addresses
            .iter()
            .find_map(|a| match a.ip() {
                IpAddr::V4(ip) => Some(ip),
                IpAddr::V6(_) => None,
            })
            .ok_or(LookupError::NoIpv4)?;
        check_join_address(SocketAddrV4::new(ip, self.port)).map_err(|_| LookupError::NotAllowed)
    }
}

/// A typed address or name, waiting on this thread for a name's lookup (a command line's join).
pub fn resolve_join_address(text: &str) -> Result<SocketAddrV4, &'static str> {
    match read_join_address(text)? {
        JoinTarget::Address(address) => Ok(address),
        JoinTarget::Name { name, port } => {
            JoinLookup::start(&name, port, system_resolver(), LOOKUP_TIMEOUT)
                .and_then(JoinLookup::wait)
                .map_err(LookupError::message)
        }
    }
}

/// The join screen's lookup in flight, and whether the player's own click or key asked for it.
struct Pending {
    lookup: JoinLookup,
    by_player: bool,
}

/// Where the join screen's lookup stands ([`JoinLookups::result`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupStatus {
    Idle,
    Running,
    /// The address, and whether the player's click or key typed the name (the connect may then reach a public address).
    Found {
        address: SocketAddrV4,
        by_player: bool,
    },
    Failed(LookupError),
}

/// The join screen's lookup: one at a time, for the text last checked.
pub struct JoinLookups {
    resolver: Resolver,
    timeout: Duration,
    slots: &'static AtomicUsize,
    pending: Option<Pending>,
}

impl JoinLookups {
    pub fn new(resolver: Resolver, timeout: Duration) -> Self {
        Self::with_slots(&LOOKUPS, resolver, timeout)
    }

    fn with_slots(slots: &'static AtomicUsize, resolver: Resolver, timeout: Duration) -> Self {
        JoinLookups {
            resolver,
            timeout,
            slots,
            pending: None,
        }
    }

    /// `checkJoinAddress`'s answer for `text` (see [`register_join_natives_with`]); `by_player` while the player's click or
    /// key is handled. The same name as the running lookup keeps it; other text cancels it.
    pub fn check(&mut self, text: &str, by_player: bool) -> String {
        let target = read_join_address(text);
        if let (Some(p), Ok(target)) = (self.pending.as_mut(), &target)
            && p.lookup.is_for(target)
        {
            p.by_player |= by_player;
            return format!("?{}", p.lookup.name());
        }
        self.pending = None;
        match target {
            Ok(JoinTarget::Address(address)) => address.to_string(),
            Ok(JoinTarget::Name { name, port }) => match JoinLookup::start_in(
                self.slots,
                &name,
                port,
                Arc::clone(&self.resolver),
                self.timeout,
            ) {
                Ok(lookup) => {
                    self.pending = Some(Pending { lookup, by_player });
                    format!("?{name}")
                }
                Err(why) => format!("!{}", why.message()),
            },
            Err(why) => format!("!{why}"),
        }
    }

    /// The lookup's state; it is dropped once it ends.
    pub fn result(&mut self) -> LookupStatus {
        let Some(p) = self.pending.as_ref() else {
            return LookupStatus::Idle;
        };
        let Some(result) = p.lookup.poll() else {
            return LookupStatus::Running;
        };
        let by_player = p.by_player;
        self.pending = None;
        match result {
            Ok(address) => LookupStatus::Found { address, by_player },
            Err(why) => LookupStatus::Failed(why),
        }
    }

    /// Drops the lookup (its thread runs on until the system answers, holding its slot).
    pub fn cancel(&mut self) {
        self.pending = None;
    }
}

/// The join screen's natives with the system's resolver (see [`register_join_natives_with`]).
pub fn register_join_natives(vm: &mut Vm, state: &Rc<RefCell<ClientState<TgeConn>>>) {
    register_join_natives_with(vm, state, system_resolver(), LOOKUP_TIMEOUT);
}

/// The join screen's natives:
/// - `checkJoinAddress(text)` gives the address as `a.b.c.d:port`, `!` and the reason it cannot be joined, or `?` and a name
///   being looked up (one at a time: other text cancels the running lookup, see [`JoinLookups::check`]);
/// - `joinLookupResult()` gives `?` while the lookup runs, then the address or `!` and the reason (`""` with none); a name the
///   player's click or key typed lets the connect that follows reach the public address it found;
/// - `cancelJoinLookup()` drops the lookup (the player left the screen);
/// - `armPublicJoin(address)` lets the connect that follows reach a public address the player typed (it does nothing unless
///   the player's own click or key is being handled, and for an address on the LAN).
pub fn register_join_natives_with(
    vm: &mut Vm,
    state: &Rc<RefCell<ClientState<TgeConn>>>,
    resolver: Resolver,
    timeout: Duration,
) {
    let lookups = Rc::new(RefCell::new(JoinLookups::new(resolver, timeout)));
    // Both reach out to an address the script gives: a name looked up, a public address the connect may reach.
    for name in ["checkJoinAddress", "armPublicJoin"] {
        vm.add_native_effects("", name, torque_ts_vm::effects::Effects::NETWORK);
    }
    let (st, running) = (Rc::clone(state), Rc::clone(&lookups));
    vm.register(
        "checkJoinAddress",
        2,
        2,
        "checkJoinAddress(text);",
        move |_, c| {
            let text = String::from_utf8_lossy(c.argv.get(1).map_or(&[][..], Vec::as_slice));
            let by_player = st.borrow().user_input;
            Native::Str(running.borrow_mut().check(&text, by_player).into_bytes())
        },
    );
    let (st, running) = (Rc::clone(state), Rc::clone(&lookups));
    vm.register(
        "joinLookupResult",
        1,
        1,
        "joinLookupResult();",
        move |vm, _| {
            let out = match running.borrow_mut().result() {
                LookupStatus::Idle => String::new(),
                LookupStatus::Running => "?".to_string(),
                LookupStatus::Found { address, by_player } => {
                    if by_player {
                        let now = vm.sim_time();
                        st.borrow().net.borrow_mut().internet.arm(address, now);
                    }
                    address.to_string()
                }
                LookupStatus::Failed(why) => format!("!{}", why.message()),
            };
            Native::Str(out.into_bytes())
        },
    );
    vm.register(
        "cancelJoinLookup",
        1,
        1,
        "cancelJoinLookup();",
        move |_, _| {
            lookups.borrow_mut().cancel();
            Native::Void
        },
    );
    let state = Rc::clone(state);
    vm.register(
        "armPublicJoin",
        2,
        2,
        "armPublicJoin(address);",
        move |vm, c| {
            let text = String::from_utf8_lossy(c.argv.get(1).map_or(&[][..], Vec::as_slice));
            let (Ok(address), now) = (parse_join_address(&text), vm.sim_time()) else {
                return Native::Void;
            };
            let st = state.borrow();
            if st.user_input {
                st.net.borrow_mut().internet.arm(address, now);
            }
            Native::Void
        },
    );
}

/// Hosting a game and joining one, on top of the rooms transport: the join screen's natives (`checkJoinAddress`,
/// `joinLookupResult`, `cancelJoinLookup` and `armPublicJoin`, [`register_join_natives`]) on every client, with the
/// resolver and the lookup's time limit the game gives (the system's by default). Not in
/// [`DefaultPlugins`](crate::plugins::DefaultPlugins): a game that lets players type an address adds it.
///
/// With the `p2p` feature it needs the rooms transport (natively the server's `RoomsServerTransportPlugin`, in a
/// browser the client's `RoomsClientTransportPlugin`), the way a player's host is found by its code and joined over data
/// channels: it is added with the plugin, and turned off, with a warning, when the rooms transport is.
#[derive(Clone, torque_engine::PluginId)]
#[plugin(id = "torque.hosting")]
pub struct HostingPlugin {
    resolver: Resolver,
    timeout: Duration,
}

impl HostingPlugin {
    /// The plugin that looks names up with `resolver`, each for at most `timeout`.
    pub fn new(resolver: Resolver, timeout: Duration) -> HostingPlugin {
        HostingPlugin { resolver, timeout }
    }
}

impl Default for HostingPlugin {
    /// The system's resolver, and [`LOOKUP_TIMEOUT`].
    fn default() -> HostingPlugin {
        HostingPlugin::new(system_resolver(), LOOKUP_TIMEOUT)
    }
}

impl Plugin for HostingPlugin {
    fn requires(&self, _requires: &mut Requires) {
        #[cfg(all(feature = "p2p", not(target_arch = "wasm32")))]
        _requires.plugin::<torque_app_server::transports::RoomsServerTransportPlugin>();
        #[cfg(all(feature = "p2p", target_arch = "wasm32"))]
        _requires.plugin::<crate::transports::RoomsClientTransportPlugin>();
    }

    fn build(&self, engine: &mut EngineBuilder) {
        let (resolver, timeout) = (Arc::clone(&self.resolver), self.timeout);
        engine.client(ClientStage::Natives, move |vm, _| {
            let wiring = client_wiring(vm, "hosting")?;
            register_join_natives_with(vm, wiring.state(), Arc::clone(&resolver), timeout);
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(
        answer: impl Fn() -> io::Result<Vec<SocketAddr>> + Send + Sync + 'static,
    ) -> Resolver {
        Arc::new(move |_, _| answer())
    }

    fn look_up(resolver: Resolver, timeout: Duration) -> Result<SocketAddrV4, LookupError> {
        static SLOTS: AtomicUsize = AtomicUsize::new(0);
        let lookup =
            JoinLookup::start_in(&SLOTS, "games.example.com", 28001, resolver, timeout).unwrap();
        loop {
            if let Some(result) = lookup.poll() {
                return result;
            }
            std::thread::yield_now();
        }
    }

    #[test]
    fn names_and_numbers_are_told_apart_and_bad_names_refused_without_a_lookup() {
        let name = |name: &str, port| {
            Ok(JoinTarget::Name {
                name: name.to_string(),
                port,
            })
        };
        assert_eq!(
            read_join_address("games.example.com"),
            name("games.example.com", 28000)
        );
        assert_eq!(read_join_address(" IP:my-box:2000 "), name("my-box", 2000));
        assert_eq!(read_join_address("localhost"), name("localhost", 28000));
        assert_eq!(
            read_join_address("host.example."),
            name("host.example.", 28000)
        );
        assert_eq!(
            read_join_address("10.0.0.5:28001"),
            Ok(JoinTarget::Address("10.0.0.5:28001".parse().unwrap()))
        );
        let long_label = format!("{}.com", "a".repeat(64));
        let long_name = ["abcdefghi"; 26].join(".");
        assert!(long_name.len() > 253);
        for bad in [
            "two words",
            "-dash.com",
            "dash-.com",
            "a..b",
            "under_score.com",
            "tab\there",
            "bell\u{7}",
            "caf\u{e9}.com",
            long_label.as_str(),
            long_name.as_str(),
        ] {
            assert_eq!(
                read_join_address(bad),
                Err(
                    "That is not an address or a name. Use one like 192.168.1.20:28000 or games.example.com."
                ),
                "{bad:?}"
            );
        }
        assert_eq!(
            read_join_address("name:0"),
            Err("Ports are numbers from 1 to 65535.")
        );
        assert_eq!(read_join_address("name:x"), Err(NOT_AN_ADDRESS));
        assert_eq!(
            read_join_address("broadcast"),
            Err("That address cannot be a game.")
        );
        assert_eq!(
            parse_join_address("name"),
            Err("That name has to be looked up first.")
        );
    }

    #[test]
    fn localhost_is_looked_up_by_the_system() {
        let at = |text| {
            let Ok(JoinTarget::Name { name, port }) = read_join_address(text) else {
                panic!("{text} is a name");
            };
            JoinLookup::start(&name, port, system_resolver(), LOOKUP_TIMEOUT)?.wait()
        };
        assert_eq!(at("localhost"), Ok("127.0.0.1:28000".parse().unwrap()));
        assert_eq!(at("localhost:2000"), Ok("127.0.0.1:2000".parse().unwrap()));
        assert_eq!(
            resolve_join_address("localhost:2001"),
            Ok("127.0.0.1:2001".parse().unwrap())
        );
    }

    #[test]
    fn a_lookup_takes_the_first_ipv4_answer_and_checks_it_like_typed_numbers() {
        let v6: SocketAddr = "[2001:db8::1]:1".parse().unwrap();
        let v4 = |ip: &str| SocketAddr::new(ip.parse().unwrap(), 1);
        let answer = |list: Vec<SocketAddr>| resolver(move || Ok(list.clone()));
        assert_eq!(
            look_up(
                answer(vec![v6, v4("203.0.113.5"), v4("10.0.0.1")]),
                LOOKUP_TIMEOUT
            ),
            Ok("203.0.113.5:28001".parse().unwrap())
        );
        assert_eq!(
            look_up(answer(vec![v6]), LOOKUP_TIMEOUT),
            Err(LookupError::NoIpv4)
        );
        assert_eq!(
            look_up(answer(vec![]), LOOKUP_TIMEOUT),
            Err(LookupError::NotFound)
        );
        for refused in [
            "0.1.2.3",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "192.168.1.255",
        ] {
            assert_eq!(
                look_up(answer(vec![v4(refused)]), LOOKUP_TIMEOUT),
                Err(LookupError::NotAllowed),
                "{refused}"
            );
        }
        let not_found = resolver(|| Err(io::Error::new(io::ErrorKind::NotFound, "no such host")));
        assert_eq!(
            look_up(not_found, LOOKUP_TIMEOUT),
            Err(LookupError::NotFound)
        );
    }

    /// A resolver that answers `slow.example` once `release` is dropped, and other names at once (203.0.113.7).
    fn slow_resolver() -> (Resolver, mpsc::Sender<()>) {
        let (release, wait) = mpsc::channel::<()>();
        let wait = std::sync::Mutex::new(wait);
        let resolver: Resolver = Arc::new(move |name, _| {
            if name == "slow.example" {
                let _ = wait.lock().unwrap().recv();
            }
            Ok(vec![SocketAddr::new(
                Ipv4Addr::new(203, 0, 113, 7).into(),
                1,
            )])
        });
        (resolver, release)
    }

    /// Waits until no lookup thread holds one of `slots`.
    fn drained(slots: &AtomicUsize) {
        let until = Instant::now() + Duration::from_secs(5);
        while slots.load(Ordering::Acquire) > 0 {
            assert!(Instant::now() < until, "the lookup threads end");
            std::thread::yield_now();
        }
    }

    fn settled(screen: &mut JoinLookups) -> LookupStatus {
        loop {
            match screen.result() {
                LookupStatus::Running => std::thread::yield_now(),
                status => return status,
            }
        }
    }

    #[test]
    fn a_slow_lookup_times_out_a_dropped_one_is_canceled_and_their_threads_are_capped() {
        static SLOTS: AtomicUsize = AtomicUsize::new(0);
        let (slow, release) = slow_resolver();
        let start =
            |name, timeout| JoinLookup::start_in(&SLOTS, name, 28000, Arc::clone(&slow), timeout);
        let lookup = start("slow.example", LOOKUP_TIMEOUT).unwrap();
        assert_eq!(lookup.poll(), None, "still running");
        let timed = start("slow.example", Duration::from_millis(30)).unwrap();
        while timed.poll().is_none() {
            std::thread::yield_now();
        }
        assert_eq!(timed.poll(), Some(Err(LookupError::TimedOut)));
        drop((lookup, timed));
        // Both threads still wait on the system: no third starts until they end.
        assert_eq!(
            start("other.example", LOOKUP_TIMEOUT).err(),
            Some(LookupError::Busy)
        );
        drop(release);
        drained(&SLOTS);
        assert!(start("other.example", LOOKUP_TIMEOUT).is_ok());
    }

    #[test]
    fn other_text_cancels_the_running_lookup_and_only_the_name_typed_at_the_click_gets_the_permit()
    {
        static SLOTS: AtomicUsize = AtomicUsize::new(0);
        let (slow, release) = slow_resolver();
        let mut screen = JoinLookups::with_slots(&SLOTS, slow, LOOKUP_TIMEOUT);
        let found = |by_player| LookupStatus::Found {
            address: "203.0.113.7:28000".parse().unwrap(),
            by_player,
        };
        assert_eq!(screen.check("slow.example", true), "?slow.example");
        assert_eq!(
            screen.check("SLOW.example:28000", false),
            "?slow.example",
            "the same name keeps its lookup"
        );
        assert_eq!(screen.result(), LookupStatus::Running);
        // Another name replaces it, and the player's click on the first name does not carry over.
        assert_eq!(screen.check("fast.example", false), "?fast.example");
        assert_eq!(settled(&mut screen), found(false));
        assert_eq!(screen.result(), LookupStatus::Idle);
        // Numbers typed meanwhile cancel it too.
        assert_eq!(screen.check("slow.example", true), "?slow.example");
        assert_eq!(screen.check("10.0.0.5", true), "10.0.0.5:28000");
        assert_eq!(screen.result(), LookupStatus::Idle);
        // The two canceled slow lookups' threads still run: the next name waits for them.
        assert_eq!(
            screen.check("fast.example", true),
            "!Still looking up the last name. Try again in a moment."
        );
        assert_eq!(screen.result(), LookupStatus::Idle);
        drop(release);
        drained(&SLOTS);
        assert_eq!(screen.check("fast.example", true), "?fast.example");
        assert_eq!(settled(&mut screen), found(true));
    }
}
