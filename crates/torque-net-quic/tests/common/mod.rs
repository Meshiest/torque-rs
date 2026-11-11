//! A server and its clients on one in-process pipe under a fake clock, stepped by hand.

#![allow(dead_code, reason = "each test file uses a different part")]

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use torque_net_quic::{
    FakeClock, Pipe, PipeIo, QuicClient, QuicLimits, QuicServer, ServerKey, Trust,
};
use torque_server::net::transport::{Recv, Transport};

pub const SERVER: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 28_000);
pub const TICK: Duration = Duration::from_millis(32);

/// xorshift64*: the tests' only randomness.
#[derive(Clone)]
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }

    pub fn percent(&mut self, p: u64) -> bool {
        self.below(100) < p
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next_u64() as u8).collect()
    }

    pub fn seed32(&mut self) -> [u8; 32] {
        let mut out = [0; 32];
        for chunk in out.chunks_mut(8) {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes());
        }
        out
    }
}

pub type Server = QuicServer<PipeIo, FakeClock>;
pub type Client = QuicClient<PipeIo, FakeClock>;

/// The server on the pipe, bound at [`SERVER`].
pub struct World {
    pub pipe: Pipe,
    pub clock: FakeClock,
    pub key: ServerKey,
    pub server: Server,
    pub limits: QuicLimits,
}

impl World {
    pub fn new(limits: QuicLimits, seed: u64) -> World {
        World::with_key(ServerKey::generate().expect("a key"), limits, seed)
    }

    pub fn with_key(key: ServerKey, limits: QuicLimits, seed: u64) -> World {
        let pipe = Pipe::new();
        let clock = FakeClock::new();
        let mut server = QuicServer::new(
            pipe.end(),
            clock.clone(),
            &key,
            limits.clone(),
            Some(Rng::new(seed).seed32()),
        )
        .expect("a server endpoint");
        server
            .bind(IpAddr::V4(*SERVER.ip()), SERVER.port())
            .expect("bound");
        World {
            pipe,
            clock,
            key,
            server,
            limits,
        }
    }

    /// A client at `addr` connecting to the server.
    pub fn client(&self, addr: SocketAddrV4, trust: Trust, seed: u64) -> Client {
        QuicClient::connect(
            self.pipe.bound(SocketAddr::V4(addr)),
            self.clock.clone(),
            SERVER,
            trust,
            &self.limits,
            Some(Rng::new(seed).seed32()),
        )
        .expect("a client endpoint")
    }

    /// Everything in flight crosses, both ends run, the clock moves one tick.
    pub fn step(&mut self, clients: &mut [&mut Client]) {
        self.pipe.deliver_all();
        self.server.pump();
        self.pipe.deliver_all();
        for client in clients.iter_mut() {
            client.pump();
        }
        self.pipe.deliver_all();
        self.server.pump();
        self.clock.advance(TICK);
    }

    /// Steps until `client` is connected (at most `ticks`).
    pub fn connect(&mut self, client: &mut Client, ticks: usize) -> bool {
        for _ in 0..ticks {
            if client.is_connected() {
                return true;
            }
            self.step(&mut [client]);
        }
        client.is_connected()
    }

    /// Steps until `client` has an error (at most `ticks`).
    pub fn fail(&mut self, client: &mut Client, ticks: usize) -> Option<String> {
        for _ in 0..ticks {
            if let Some(e) = client.error() {
                return Some(e);
            }
            self.step(&mut [client]);
        }
        client.error()
    }

    /// Every application datagram the server has for the host.
    pub fn server_recv(&mut self) -> Vec<(SocketAddr, Vec<u8>)> {
        let mut buf = vec![0; 1 << 16];
        let mut out = Vec::new();
        loop {
            match self.server.recv(&mut buf) {
                Recv::Datagram { len, from } => out.push((from, buf[..len].to_vec())),
                Recv::Skipped => {}
                Recv::Empty => return out,
            }
        }
    }
}

/// Every application datagram `client` has.
pub fn client_recv(client: &mut Client) -> Vec<Vec<u8>> {
    use torque_client::net::Datagrams;
    let mut out = Vec::new();
    while let Some((_, data)) = client.recv() {
        out.push(data);
    }
    out
}
