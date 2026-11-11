//! Where an endpoint's datagrams go in and out: a non-blocking UDP socket, or an in-process pipe the tests hold both ends of
//! and mangle what crosses it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::rc::Rc;

use torque_server::net::transport::{BindError, BindPolicy};

/// The datagram socket under an endpoint. Nothing blocks.
pub trait Io {
    /// Whether this is an operating-system socket (a host with one keeps the system clock).
    const REAL: bool;

    /// Opens the socket on `ip:port` (0: any free port), closing the one it had.
    ///
    /// # Errors
    /// The bind policy refuses the address, or the system refused the bind.
    fn bind(&mut self, ip: IpAddr, port: u16) -> Result<(), BindError>;
    /// The address the socket is bound to, if it is.
    fn local_addr(&self) -> Option<SocketAddr>;
    /// Sends one datagram; false when it was not sent.
    fn send(&mut self, to: SocketAddr, data: &[u8]) -> bool;
    /// The next datagram into `buf`, or none waiting. A datagram the system reported an error for is skipped and counted.
    fn recv(&mut self, buf: &mut [u8]) -> Option<(usize, SocketAddr)>;
    /// Datagrams the receives lost (an error for one datagram), so far.
    fn lost(&self) -> u64;
    /// Closes the socket; it stays closed until the next `bind`.
    fn close(&mut self);
}

/// One non-blocking UDP socket, bound under a bind policy when it has one (a server), anywhere when it has none (a client).
pub struct UdpIo {
    socket: Option<UdpSocket>,
    policy: Option<BindPolicy>,
    lost: u64,
}

impl UdpIo {
    /// A socket that binds only where `policy` allows.
    #[must_use]
    pub fn new(policy: BindPolicy) -> UdpIo {
        UdpIo {
            socket: None,
            policy: Some(policy),
            lost: 0,
        }
    }

    /// A socket with no bind policy (a client's), bound on `ip` at `port` (0: any free port).
    ///
    /// # Errors
    /// The system refused the bind.
    pub fn open(ip: IpAddr, port: u16) -> Result<UdpIo, BindError> {
        let mut io = UdpIo {
            socket: None,
            policy: None,
            lost: 0,
        };
        io.bind(ip, port)?;
        Ok(io)
    }
}

impl Io for UdpIo {
    const REAL: bool = true;

    fn bind(&mut self, ip: IpAddr, port: u16) -> Result<(), BindError> {
        if let Some(policy) = &self.policy {
            policy.check(ip).map_err(BindError::Policy)?;
        }
        self.socket = None;
        let socket =
            UdpSocket::bind(SocketAddr::new(ip, port)).map_err(|e| BindError::Os(e.kind()))?;
        socket
            .set_nonblocking(true)
            .map_err(|e| BindError::Os(e.kind()))?;
        self.socket = Some(socket);
        Ok(())
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.socket.as_ref().and_then(|s| s.local_addr().ok())
    }

    fn send(&mut self, to: SocketAddr, data: &[u8]) -> bool {
        self.socket
            .as_ref()
            .is_some_and(|s| s.send_to(data, to).is_ok())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Option<(usize, SocketAddr)> {
        let socket = self.socket.as_ref()?;
        loop {
            match socket.recv_from(buf) {
                Ok(got) => return Some(got),
                Err(e) if e.kind() == ErrorKind::WouldBlock => return None,
                // Windows reports a send to a closed port on the next receive: counted, and the next datagram is asked for.
                Err(_) => self.lost += 1,
            }
        }
    }

    fn lost(&self) -> u64 {
        self.lost
    }

    fn close(&mut self) {
        self.socket = None;
    }
}

/// A datagram on its way through a [`Pipe`], not yet delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flight {
    pub from: SocketAddr,
    pub to: SocketAddr,
    pub data: Vec<u8>,
}

#[derive(Default)]
struct Net {
    inboxes: HashMap<SocketAddr, VecDeque<(SocketAddr, Vec<u8>)>>,
    in_flight: Vec<Flight>,
    bound: HashSet<SocketAddr>,
    next_port: u16,
}

/// An in-process network: every send goes in flight, and the test delivers what it chooses, as it chooses
/// ([`Pipe::deliver`], [`Pipe::deliver_all`]). Nothing crosses it on its own.
#[derive(Clone, Default)]
pub struct Pipe(Rc<RefCell<Net>>);

impl Pipe {
    #[must_use]
    pub fn new() -> Pipe {
        Pipe::default()
    }

    /// A socket on this network, not yet bound.
    #[must_use]
    pub fn end(&self) -> PipeIo {
        PipeIo {
            net: Rc::clone(&self.0),
            addr: None,
            lost: 0,
        }
    }

    /// A socket bound at `addr`.
    #[must_use]
    pub fn bound(&self, addr: SocketAddr) -> PipeIo {
        let mut io = self.end();
        io.bind(addr.ip(), addr.port())
            .expect("a pipe binds anywhere");
        io
    }

    /// Everything sent and not yet taken, in send order.
    pub fn take_in_flight(&self) -> Vec<Flight> {
        std::mem::take(&mut self.0.borrow_mut().in_flight)
    }

    /// Datagrams in flight, not yet taken.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.0.borrow().in_flight.len()
    }

    /// Puts `flight` in the inbox of its destination (dropped if no socket is bound there).
    pub fn deliver(&self, flight: Flight) {
        let mut net = self.0.borrow_mut();
        if let Some(inbox) = net.inboxes.get_mut(&flight.to) {
            inbox.push_back((flight.from, flight.data));
        }
    }

    /// Delivers everything in flight, in order.
    pub fn deliver_all(&self) {
        for flight in self.take_in_flight() {
            self.deliver(flight);
        }
    }
}

/// One socket of a [`Pipe`].
pub struct PipeIo {
    net: Rc<RefCell<Net>>,
    addr: Option<SocketAddr>,
    lost: u64,
}

impl Drop for PipeIo {
    /// A dropped socket gives its address back, as a closed one does.
    fn drop(&mut self) {
        self.close();
    }
}

impl Io for PipeIo {
    const REAL: bool = false;

    fn bind(&mut self, ip: IpAddr, port: u16) -> Result<(), BindError> {
        self.close();
        let mut net = self.net.borrow_mut();
        let port = if port == 0 {
            net.next_port = net.next_port.wrapping_add(1).max(1);
            40_000 + net.next_port % 20_000
        } else {
            port
        };
        let addr = SocketAddr::new(ip, port);
        if !net.bound.insert(addr) {
            return Err(BindError::Os(ErrorKind::AddrInUse));
        }
        net.inboxes.entry(addr).or_default();
        self.addr = Some(addr);
        Ok(())
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.addr
    }

    fn send(&mut self, to: SocketAddr, data: &[u8]) -> bool {
        let Some(from) = self.addr else {
            return false;
        };
        self.net.borrow_mut().in_flight.push(Flight {
            from,
            to,
            data: data.to_vec(),
        });
        true
    }

    fn recv(&mut self, buf: &mut [u8]) -> Option<(usize, SocketAddr)> {
        let addr = self.addr?;
        let mut net = self.net.borrow_mut();
        let (from, data) = net.inboxes.get_mut(&addr)?.pop_front()?;
        if data.len() > buf.len() {
            self.lost += 1;
            return None;
        }
        buf[..data.len()].copy_from_slice(&data);
        Some((data.len(), from))
    }

    fn lost(&self) -> u64 {
        self.lost
    }

    fn close(&mut self) {
        if let Some(addr) = self.addr.take() {
            let mut net = self.net.borrow_mut();
            net.bound.remove(&addr);
            net.inboxes.remove(&addr);
        }
    }
}
