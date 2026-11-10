//! The sockets: a connect on a thread of its own ([`start`]: the name lookup, the connect and, for `https://`, the TLS
//! handshake, so a tick never waits on the system), and [`Link`], an open connection read and written without blocking.

use std::io::{self, ErrorKind, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use crate::address::{Host, Target};
use crate::limits::MAX_CONNECT_THREADS;

/// Looks up a host name's addresses (`name`, `port`): the system's ([`system_resolver`]) or a test's.
pub type Resolver = Arc<dyn Fn(&str, u16) -> io::Result<Vec<SocketAddr>> + Send + Sync>;

/// The system's resolver (its hosts file, then DNS).
pub fn system_resolver() -> Resolver {
    use std::net::ToSocketAddrs;
    Arc::new(|name, port| Ok((name, port).to_socket_addrs()?.collect()))
}

/// What a connect thread reports, in this order: `Resolved` (only for a name), then one of the others.
pub(crate) enum Progress {
    Resolved,
    Connected(Box<Link>),
    DnsFailed,
    ConnectFailed,
}

/// How connects are made: the resolver, and the TLS settings of `https://` addresses.
#[derive(Clone)]
pub(crate) struct Connector {
    pub(crate) resolver: Resolver,
    #[cfg(feature = "https")]
    pub(crate) tls: crate::tls::TlsSettings,
}

/// The connect threads alive, for every script engine of the process.
static THREADS: AtomicUsize = AtomicUsize::new(0);

/// One of [`MAX_CONNECT_THREADS`], given back when the thread holding it ends (or never starts).
struct Slot;

impl Slot {
    fn take() -> Option<Slot> {
        THREADS
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_CONNECT_THREADS).then_some(n + 1)
            })
            .ok()
            .map(|_| Slot)
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        THREADS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// One of a script engine's live connect threads, given back when the thread ends (or never starts).
struct Live(Arc<AtomicUsize>);

impl Live {
    fn new(count: Arc<AtomicUsize>) -> Live {
        count.fetch_add(1, Ordering::AcqRel);
        Live(count)
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Every connect thread of the process is busy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Busy;

/// Starts the connect to `target` on a thread of its own, which gives up after `within`; its reports come on the receiver.
/// Dropping the receiver cancels it (a thread waiting on the system's lookup ends when the system answers). `live` counts
/// the engine's threads until each ends.
pub(crate) fn start(
    connector: &Connector,
    target: Target,
    within: Duration,
    live: Arc<AtomicUsize>,
) -> Result<Receiver<Progress>, Busy> {
    let slot = Slot::take().ok_or(Busy)?;
    let live = Live::new(live);
    let (tx, rx) = mpsc::channel();
    let connector = connector.clone();
    // A thread that cannot start drops `tx`: the connect ends as failed.
    let _ = std::thread::Builder::new()
        .name("tcpobject-connect".into())
        .spawn(move || {
            connect(&connector, &target, within, &tx);
            drop((slot, live));
        });
    Ok(rx)
}

/// The first IPv4 address, else the first (TGE looks up IPv4 only).
fn pick(addresses: &[SocketAddr]) -> Option<SocketAddr> {
    addresses
        .iter()
        .find(|a| a.is_ipv4())
        .or_else(|| addresses.first())
        .copied()
}

fn connect(connector: &Connector, target: &Target, within: Duration, tx: &Sender<Progress>) {
    let deadline = Instant::now() + within;
    let address = match &target.host {
        Host::Ip(ip) => SocketAddr::new(IpAddr::V4(*ip), target.port),
        Host::Name(name) => {
            let found = (connector.resolver)(name, target.port)
                .ok()
                .and_then(|found| pick(&found));
            let Some(address) = found else {
                let _ = tx.send(Progress::DnsFailed);
                return;
            };
            if tx.send(Progress::Resolved).is_err() {
                return;
            }
            address
        }
        Host::Unusable => {
            let _ = tx.send(Progress::DnsFailed);
            return;
        }
    };
    let progress = open(connector, target, address, deadline)
        .map_or(Progress::ConnectFailed, |link| {
            Progress::Connected(Box::new(link))
        });
    let _ = tx.send(progress);
}

/// The time left before `deadline`, or `None` once it passed.
fn left(deadline: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
}

fn open(
    connector: &Connector,
    target: &Target,
    address: SocketAddr,
    deadline: Instant,
) -> io::Result<Link> {
    let timed_out = || io::Error::from(ErrorKind::TimedOut);
    let tcp = TcpStream::connect_timeout(&address, left(deadline).ok_or_else(timed_out)?)?;
    let _ = tcp.set_nodelay(true);
    let link = if target.tls {
        #[cfg(feature = "https")]
        {
            crate::tls::handshake(&connector.tls, tcp, &target.host.text(), deadline)?
        }
        #[cfg(not(feature = "https"))]
        {
            let _ = connector;
            return Err(io::Error::new(
                ErrorKind::Unsupported,
                "built without the https feature",
            ));
        }
    } else {
        Link::Plain(tcp)
    };
    link.tcp().set_nonblocking(true)?;
    Ok(link)
}

/// An open connection, plain or under TLS, that never blocks: a read or write that cannot go on now is `WouldBlock`.
pub(crate) enum Link {
    Plain(TcpStream),
    #[cfg(feature = "https")]
    Tls(Box<crate::tls::TlsWire>),
}

impl Link {
    pub(crate) fn tcp(&self) -> &TcpStream {
        match self {
            Link::Plain(tcp) => tcp,
            #[cfg(feature = "https")]
            Link::Tls(tls) => &tls.tcp,
        }
    }

    /// Bytes the peer sent: `Ok(0)` once it closed the connection.
    pub(crate) fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Link::Plain(tcp) => tcp.read(buffer),
            #[cfg(feature = "https")]
            Link::Tls(tls) => tls.read(buffer),
        }
    }

    /// Writes what it can of `data` now.
    pub(crate) fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        match self {
            Link::Plain(tcp) => tcp.write(data),
            #[cfg(feature = "https")]
            Link::Tls(tls) => tls.write(data),
        }
    }

    /// Ends the connection: what was written is sent, then the socket closes.
    pub(crate) fn close(&mut self) {
        #[cfg(feature = "https")]
        if let Link::Tls(tls) = self {
            tls.close();
        }
        let _ = self.tcp().shutdown(Shutdown::Write);
    }
}

/// Whether an error from a read or write only means "not now".
pub(crate) fn would_block(error: &io::Error) -> bool {
    matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted)
}
