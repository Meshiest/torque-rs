//! An in-process stand-in for the loopback network, for a scripted session whose pictures must come out the same on every
//! run: a datagram is in its receiver's queue the moment it is sent, so each read sees exactly what was sent before it.
//!
//! **Why.** The system's loopback makes no such promise: under load it may hold a datagram past the receiver's next read. One
//! datagram read a frame later moves the session off its script for good: a move reaches the server a tick late, a teleport is
//! caught on another frame, a wait for a bolt ends a frame later, and from then on the figures' animations and the round's
//! clock stand somewhere else when each picture is taken. Over this network the clients and the server they share run as they
//! would on a loopback that never held a datagram back.
//!
//! **The parts.** Both register under [`INSTANT_NET`], and neither is in the engine's default groups: a harness adds them to
//! the engine it builds. [`InstantNetClientTransportPlugin`] connects a client to an [`InstantNet`] (the target): a socket
//! ([`InstantNet::socket`]) on the next free port of `127.0.0.1`, read under [`NetPolicy::Loopback`].
//! [`InstantNetServerTransportPlugin`] takes the server's transport [`InstantNet::listen`] bound ([`Listen::Bound`]). Unlike
//! the `memory` link ([`MemoryTarget`](super::MemoryTarget)), which carries one client to one server when a step calls for
//! it, any number of clients and servers share the network and nothing has to carry their datagrams.
//!
//! **Delivery.** A socket is bound at an IPv4 address and a port, one socket to a port. A datagram reaches the socket on its
//! port bound at its address or at `0.0.0.0`; one to `255.255.255.255` reaches every socket on its port but the sender's (the
//! LAN list's query). A datagram that reaches no socket (nothing bound there, or the receiver's queue already holds
//! [`QUEUE_MAX`] datagrams) is dropped, and its send says so. A socket bound at `0.0.0.0` sends from `127.0.0.1`.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::rc::Rc;

use torque_client::net::{Datagrams, NetPolicy};
use torque_engine::{EngineBuilder, Plugin};
use torque_server::net::transport::{BindError, Recv, SharedTransport, Transport};

use super::{
    BoundTransport, ClientLink, EngineClientTransports, EngineServerTransports, bound_transport,
};
use crate::server::Listen;

/// The id both halves are registered under ([`InstantNetClientTransportPlugin`], [`InstantNetServerTransportPlugin`]).
pub const INSTANT_NET: &str = "instant-net";
/// The most datagrams a socket's queue holds unread.
pub const QUEUE_MAX: usize = 4096;
/// The first port a socket bound to port 0 may get.
const FIRST_FREE_PORT: u16 = 49152;

/// A datagram waiting to be read: who sent it, and its bytes.
type Datagram = (SocketAddrV4, Vec<u8>);

/// A bound socket: its address and the datagrams waiting for it.
struct Bound {
    ip: Ipv4Addr,
    queue: VecDeque<Datagram>,
}

/// The network's sockets, by port.
#[derive(Default)]
struct Wires {
    ports: BTreeMap<u16, Bound>,
}

impl Wires {
    /// Binds `ip` and `port` (the next free one for 0).
    fn bind(&mut self, ip: Ipv4Addr, port: u16) -> Result<SocketAddrV4, ErrorKind> {
        let port = match port {
            0 => (FIRST_FREE_PORT..=u16::MAX)
                .find(|p| !self.ports.contains_key(p))
                .ok_or(ErrorKind::AddrNotAvailable)?,
            p if self.ports.contains_key(&p) => return Err(ErrorKind::AddrInUse),
            p => p,
        };
        let queue = VecDeque::new();
        self.ports.insert(port, Bound { ip, queue });
        Ok(SocketAddrV4::new(ip, port))
    }

    fn close(&mut self, at: SocketAddrV4) {
        self.ports.remove(&at.port());
    }

    /// Puts `data` from the socket at `from` in the queue of the socket `to` reaches (see the module docs); whether it did.
    fn send(&mut self, from: SocketAddrV4, to: SocketAddrV4, data: &[u8]) -> bool {
        let source = if from.ip().is_unspecified() {
            SocketAddrV4::new(Ipv4Addr::LOCALHOST, from.port())
        } else {
            from
        };
        let Some(bound) = self.ports.get_mut(&to.port()) else {
            return false;
        };
        let reached = if *to.ip() == Ipv4Addr::BROADCAST {
            to.port() != from.port()
        } else {
            bound.ip == *to.ip() || bound.ip.is_unspecified()
        };
        if !reached || bound.queue.len() >= QUEUE_MAX {
            return false;
        }
        bound.queue.push_back((source, data.to_vec()));
        true
    }

    fn take(&mut self, at: SocketAddrV4) -> Option<Datagram> {
        self.ports.get_mut(&at.port())?.queue.pop_front()
    }
}

/// An in-process network the sockets and transports it makes share (see the module docs). Cloning it shares the network.
#[derive(Clone, Default)]
pub struct InstantNet(Rc<RefCell<Wires>>);

impl fmt::Debug for InstantNet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bound: Vec<SocketAddrV4> = (self.0.borrow().ports.iter())
            .map(|(&port, b)| SocketAddrV4::new(b.ip, port))
            .collect();
        f.debug_tuple("InstantNet").field(&bound).finish()
    }
}

impl InstantNet {
    /// A client's socket on the next free port of `127.0.0.1`.
    ///
    /// # Errors
    /// Every port is taken.
    pub fn socket(&self) -> Result<Box<dyn Datagrams>, String> {
        let at = self
            .0
            .borrow_mut()
            .bind(Ipv4Addr::LOCALHOST, 0)
            .map_err(|e| format!("no free port on the in-process network: {e}"))?;
        Ok(Box::new(InstantSocket {
            wires: Rc::clone(&self.0),
            at,
        }))
    }

    /// A server's transport bound at `ip` on `port` (0: the next free one), as the server start takes it.
    ///
    /// # Errors
    /// `port` is taken (or, for 0, every port is).
    pub fn listen(&self, ip: Ipv4Addr, port: u16) -> Result<Listen, String> {
        let mut transport = InstantTransport {
            wires: Rc::clone(&self.0),
            at: None,
        };
        transport
            .bind(IpAddr::V4(ip), port)
            .map_err(|e| format!("the server's transport: {e}"))?;
        let transport: SharedTransport = Rc::new(RefCell::new(transport));
        Ok(Listen::Bound(BoundTransport::new(INSTANT_NET, transport)))
    }
}

/// A client's socket on an [`InstantNet`].
struct InstantSocket {
    wires: Rc<RefCell<Wires>>,
    at: SocketAddrV4,
}

impl Datagrams for InstantSocket {
    fn send_to(&mut self, to: SocketAddrV4, bytes: &[u8]) -> bool {
        self.wires.borrow_mut().send(self.at, to, bytes)
    }

    fn recv(&mut self) -> Option<(SocketAddrV4, Vec<u8>)> {
        self.wires.borrow_mut().take(self.at)
    }

    fn port(&self) -> u16 {
        self.at.port()
    }
}

impl Drop for InstantSocket {
    fn drop(&mut self) {
        self.wires.borrow_mut().close(self.at);
    }
}

/// A server's transport on an [`InstantNet`].
struct InstantTransport {
    wires: Rc<RefCell<Wires>>,
    at: Option<SocketAddrV4>,
}

impl Transport for InstantTransport {
    /// Binds `ip` (IPv4 only: the network has no other) on `port`.
    fn bind(&mut self, ip: IpAddr, port: u16) -> Result<(), BindError> {
        let IpAddr::V4(ip) = ip else {
            return Err(BindError::Os(ErrorKind::AddrNotAvailable));
        };
        self.close();
        self.at = Some(
            self.wires
                .borrow_mut()
                .bind(ip, port)
                .map_err(BindError::Os)?,
        );
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Recv {
        let Some((from, data)) = self.at.and_then(|at| self.wires.borrow_mut().take(at)) else {
            return Recv::Empty;
        };
        // A datagram longer than the buffer is cut, as the system cuts it.
        let len = data.len().min(buf.len());
        buf[..len].copy_from_slice(&data[..len]);
        Recv::Datagram {
            len,
            from: SocketAddr::V4(from),
        }
    }

    fn send_to(&mut self, to: SocketAddr, data: &[u8]) -> bool {
        let (Some(at), SocketAddr::V4(to)) = (self.at, to) else {
            return false;
        };
        self.wires.borrow_mut().send(at, to, data)
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.at.map(SocketAddr::V4)
    }

    fn is_real(&self) -> bool {
        false
    }

    fn close(&mut self) {
        if let Some(at) = self.at.take() {
            self.wires.borrow_mut().close(at);
        }
    }
}

impl Drop for InstantTransport {
    fn drop(&mut self) {
        self.close();
    }
}

/// A client's socket on an [`InstantNet`], the target it connects to. Registers [`INSTANT_NET`].
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.transport-instant-net-client")]
pub struct InstantNetClientTransportPlugin;

impl Plugin for InstantNetClientTransportPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_client_transport(INSTANT_NET, |net: &InstantNet| {
            Ok(ClientLink::new(net.socket()?, NetPolicy::Loopback))
        });
    }
}

/// A server on an [`InstantNet`]: takes the transport [`InstantNet::listen`] bound. Registers [`INSTANT_NET`].
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.transport-instant-net-server")]
pub struct InstantNetServerTransportPlugin;

impl Plugin for InstantNetServerTransportPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_server_transport(INSTANT_NET, bound_transport(INSTANT_NET));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to(port: u16) -> SocketAddrV4 {
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)
    }

    #[test]
    fn a_datagram_is_there_at_the_next_read() {
        let net = InstantNet::default();
        let (mut a, mut b) = (net.socket().unwrap(), net.socket().unwrap());
        assert_ne!(a.port(), b.port());
        assert!(a.send_to(to(b.port()), b"one"));
        assert!(a.send_to(to(b.port()), b"two"));
        assert_eq!(b.recv(), Some((to(a.port()), b"one".to_vec())));
        assert_eq!(b.recv(), Some((to(a.port()), b"two".to_vec())));
        assert_eq!(b.recv(), None);
        assert_eq!(a.recv(), None);
    }

    #[test]
    fn the_server_hears_a_broadcast_on_its_port_and_answers() {
        let net = InstantNet::default();
        let Listen::Bound(bound) = net.listen(Ipv4Addr::UNSPECIFIED, 0).unwrap() else {
            panic!("a bound transport");
        };
        let server = bound.transport;
        let Some(SocketAddr::V4(at)) = server.borrow().local_addr() else {
            panic!("the transport is bound");
        };
        let mut client = net.socket().unwrap();
        assert!(client.send_to(SocketAddrV4::new(Ipv4Addr::BROADCAST, at.port()), b"query"));
        assert!(!client.send_to(to(at.port() + 1000), b"nobody"));
        assert!(!client.send_to(
            SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), client.port()),
            b"nobody"
        ));
        let mut buf = [0u8; 16];
        let Recv::Datagram { len, from } = server.borrow_mut().recv(&mut buf) else {
            panic!("the query arrived");
        };
        assert_eq!(
            (&buf[..len], from),
            (&b"query"[..], SocketAddr::V4(to(client.port())))
        );
        assert_eq!(server.borrow_mut().recv(&mut buf), Recv::Empty);
        assert!(server.borrow_mut().send_to(from, b"answer"));
        assert_eq!(*at.ip(), Ipv4Addr::UNSPECIFIED);
        assert_eq!(client.recv(), Some((to(at.port()), b"answer".to_vec())));
    }

    #[test]
    fn a_closed_port_is_free_again() {
        let net = InstantNet::default();
        let port = {
            let socket = net.socket().unwrap();
            assert!(net.listen(Ipv4Addr::LOCALHOST, socket.port()).is_err());
            socket.port()
        };
        assert!(net.listen(Ipv4Addr::LOCALHOST, port).is_ok());
    }
}
