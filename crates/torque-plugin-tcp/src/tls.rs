//! HTTPS for HTTPObject: rustls with the ring provider, the certificate checked against the system's trusted roots (read once
//! per process, on the first connect thread that needs them) or the roots a game gives.

use std::io::{self, ErrorKind, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore};

use crate::link::Link;

/// TLS reads and record work one [`TlsWire::read`] may do before it answers "not now": a peer's flood of empty records
/// cannot keep a tick in one read.
const MAX_READ_ROUNDS: usize = 64;

/// Where the TLS settings come from.
#[derive(Clone, Default)]
pub(crate) enum TlsSettings {
    /// The system's trusted roots, read on first use.
    #[default]
    System,
    /// A game's own settings.
    Given(Arc<ClientConfig>),
}

/// The client settings with the system's roots, made once.
static SYSTEM: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();

/// The settings over `roots`, with the ring provider and its safe default versions.
pub fn client_config(roots: RootCertStore) -> Result<ClientConfig, rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Ok(ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth())
}

fn system() -> Result<Arc<ClientConfig>, String> {
    SYSTEM
        .get_or_init(|| {
            let mut roots = RootCertStore::empty();
            let found = rustls_native_certs::load_native_certs();
            let (added, _) = roots.add_parsable_certificates(found.certs);
            if added == 0 {
                return Err("no trusted root certificates on this system".to_string());
            }
            client_config(roots)
                .map(Arc::new)
                .map_err(|e| e.to_string())
        })
        .clone()
}

impl TlsSettings {
    fn config(&self) -> io::Result<Arc<ClientConfig>> {
        match self {
            TlsSettings::System => system().map_err(io::Error::other),
            TlsSettings::Given(config) => Ok(Arc::clone(config)),
        }
    }
}

/// The handshake over `tcp` with `host`, on the connect thread, given up at `deadline`.
pub(crate) fn handshake(
    settings: &TlsSettings,
    mut tcp: TcpStream,
    host: &str,
    deadline: Instant,
) -> io::Result<Link> {
    let name = ServerName::try_from(host.to_string())
        .map_err(|e| io::Error::new(ErrorKind::InvalidInput, e))?;
    let mut conn = ClientConnection::new(settings.config()?, name).map_err(io::Error::other)?;
    while conn.is_handshaking() {
        let left = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| io::Error::from(ErrorKind::TimedOut))?;
        tcp.set_read_timeout(Some(left))?;
        tcp.set_write_timeout(Some(left))?;
        conn.complete_io(&mut tcp)?;
    }
    tcp.set_read_timeout(None)?;
    tcp.set_write_timeout(None)?;
    Ok(Link::Tls(Box::new(TlsWire { tcp, conn })))
}

/// An open TLS connection on a socket that does not block.
pub(crate) struct TlsWire {
    pub(crate) tcp: TcpStream,
    conn: ClientConnection,
}

impl TlsWire {
    /// Plaintext the peer sent: `Ok(0)` once it closed (with or without its close notice).
    pub(crate) fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        for _ in 0..MAX_READ_ROUNDS {
            match self.conn.reader().read(buffer) {
                Ok(n) => return Ok(n),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(0),
                Err(e) => return Err(e),
            }
            let got = self.conn.read_tls(&mut self.tcp)?;
            self.conn
                .process_new_packets()
                .map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?;
            // An alert or a key update may want an answer.
            self.flush()?;
            if got == 0 {
                return match self.conn.reader().read(buffer) {
                    Ok(n) => Ok(n),
                    Err(_) => Ok(0),
                };
            }
        }
        Err(ErrorKind::WouldBlock.into())
    }

    /// Takes what of `data` fits beside what still waits to go out, and sends what it can.
    pub(crate) fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.flush()?;
        if self.conn.wants_write() {
            return Err(ErrorKind::WouldBlock.into());
        }
        let taken = self.conn.writer().write(data)?;
        self.flush()?;
        Ok(taken)
    }

    /// Sends the records that wait, as far as the socket takes them now.
    fn flush(&mut self) -> io::Result<()> {
        while self.conn.wants_write() {
            match self.conn.write_tls(&mut self.tcp) {
                Ok(0) => return Err(ErrorKind::WriteZero.into()),
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(()),
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Sends the close notice (as far as the socket takes it).
    pub(crate) fn close(&mut self) {
        self.conn.send_close_notify();
        let _ = self.flush();
    }
}
