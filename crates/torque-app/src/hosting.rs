//! Hosting a game and joining one, as a game's client does it: where its socket binds, which address a hosted game binds and
//! is joined by, and the address a player types.
//!
//! - [`NetMode`]: [`NetMode::Loopback`] binds the client's socket to `127.0.0.1` and reads only loopback datagrams (tests, a
//!   game for one); [`NetMode::Lan`] binds it as TGE binds one (every address, with a loopback twin so a server on `127.0.0.1`
//!   hears its LAN query) and talks to LAN addresses only, but for one public address the player typed ([`register_join_natives`]).
//! - A hosted game binds this computer's LAN address ([`lan_bind_address`], found from the system's routing, or the one the
//!   player named), and others on the network join it by one of [`lan_reach`]; the hosting client joins its own server by
//!   [`join_address`].
//! - [`parse_join_address`] reads a typed address; [`register_join_natives`] gives the scripts `checkJoinAddress` and
//!   `armPublicJoin`.

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::rc::Rc;

use super::client::ClientNet;
use super::server::lan_addresses;
use torque_client::conn::tge::TgeConn;
use torque_client::state::ClientState;
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

/// Why a typed address cannot be joined, or the address: `a.b.c.d:port`, a bare `a.b.c.d` (port [`DEFAULT_PORT`](super::server::DEFAULT_PORT)) or `localhost`,
/// with or without an `IP:` in front. The engine resolves no names, so a host name gets a plain message asking for the numbers.
pub fn parse_join_address(text: &str) -> Result<SocketAddrV4, &'static str> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Type the game's address, like 192.168.1.20:28000.");
    }
    let Some(address) = torque_client::net::query::parse_address(text.as_bytes()) else {
        let name = !text.contains(':') && text.chars().any(|c| c.is_ascii_alphabetic());
        return Err(if name {
            "Names are not looked up. Type the address in numbers, like 192.168.1.20."
        } else {
            "That is not an address. Use numbers like 192.168.1.20:28000."
        });
    };
    let ip = *address.ip();
    if address.port() == 0 {
        return Err("Ports are numbers from 1 to 65535.");
    }
    // Not a game's address: this network (0/8), multicast, reserved (240/4) and broadcasts (a last number of 255, as on a /24 LAN).
    if ip.octets()[0] == 0 || ip.octets()[0] >= 224 || ip.octets()[3] == 255 {
        return Err("That address cannot be a game.");
    }
    Ok(address)
}

/// The join screen's natives: `checkJoinAddress(text)` gives the address as `a.b.c.d:port`, or `!` and the reason it cannot be
/// joined; `armPublicJoin(address)` lets the connect that follows reach a public address the player typed (it does nothing
/// unless the player's own click or key is being handled, and for an address on the LAN).
pub fn register_join_natives(vm: &mut Vm, state: &Rc<RefCell<ClientState<TgeConn>>>) {
    vm.register(
        "checkJoinAddress",
        2,
        2,
        "checkJoinAddress(text);",
        |_, c| {
            let text = String::from_utf8_lossy(c.argv.get(1).map_or(&[][..], Vec::as_slice));
            Native::Str(
                match parse_join_address(&text) {
                    Ok(address) => address.to_string(),
                    Err(why) => format!("!{why}"),
                }
                .into_bytes(),
            )
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
