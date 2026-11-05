//! TLS for the web transport (`wss` and `https`): a certificate chain and its key read from PEM files, rustls with the ring
//! provider, and the byte stream a connection reads and writes, plain or under TLS, metered on the wire.

use std::io::{self, ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection};
use web_time::{Duration, Instant};

/// The PEM files of a server's certificate chain and its private key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsFiles {
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// The most bytes a PEM file may have.
pub const MAX_PEM_BYTES: u64 = 1 << 20;

/// A server's TLS settings, ready for connections.
#[derive(Clone)]
pub struct Tls {
    config: Arc<ServerConfig>,
}

impl std::fmt::Debug for Tls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tls")
    }
}

impl Tls {
    /// Reads the certificate chain and the key `files` name.
    pub fn load(files: &TlsFiles) -> Result<Tls, String> {
        let read = |path: &PathBuf, what: &str| -> Result<Vec<u8>, String> {
            let len = std::fs::metadata(path)
                .map_err(|e| format!("the {what} {}: {e}", path.display()))?
                .len();
            if len > MAX_PEM_BYTES {
                return Err(format!("the {what} {} is too large", path.display()));
            }
            std::fs::read(path).map_err(|e| format!("the {what} {}: {e}", path.display()))
        };
        Tls::from_pem(
            &read(&files.cert, "certificate")?,
            &read(&files.key, "key")?,
        )
    }

    /// The settings for a certificate chain and a key in PEM text.
    pub fn from_pem(cert: &[u8], key: &[u8]) -> Result<Tls, String> {
        let chain = CertificateDer::pem_slice_iter(cert)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("the certificate: {e}"))?;
        if chain.is_empty() {
            return Err("the certificate file holds no certificate".to_string());
        }
        let key = PrivateKeyDer::from_pem_slice(key).map_err(|e| format!("the key: {e}"))?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| format!("TLS: {e}"))?
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .map_err(|e| format!("the certificate and key: {e}"))?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Tls {
            config: Arc::new(config),
        })
    }

    pub(crate) fn wrap(&self, tcp: TcpStream, caps: WireCaps, now: Instant) -> io::Result<Pipe> {
        let conn = ServerConnection::new(Arc::clone(&self.config))
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(Pipe::new(tcp, Some(Box::new(conn)), caps, now))
    }
}

/// The most bytes one TLS step reads from the socket (rustls itself asks for at most 4 KiB a read outside a handshake).
const TLS_READ_CHUNK: usize = 16 << 10;
/// The most socket writes one call makes to send what TLS has queued.
const TLS_WRITES_PER_CALL: usize = 4;

/// What a connection may read off the wire (the socket's bytes, encrypted ones under TLS): so many bytes in one pass, so
/// many in a second.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WireCaps {
    pub bytes_per_pass: usize,
    pub bytes_per_s: usize,
}

/// The wire meter: a pass's budget, and the bytes of the present second.
struct Wire {
    caps: WireCaps,
    budget: usize,
    window: Instant,
    bytes: usize,
    over: bool,
}

impl Wire {
    /// The bytes the next read may take (0 once over the cap or out of budget).
    fn room(&self) -> usize {
        if self.over { 0 } else { self.budget }
    }

    fn count(&mut self, n: usize) {
        self.budget = self.budget.saturating_sub(n);
        self.bytes += n;
        if self.bytes > self.caps.bytes_per_s {
            self.over = true;
        }
    }
}

/// A socket read held to the wire meter's room, counted on it.
struct Limited<'a> {
    tcp: &'a mut TcpStream,
    wire: &'a mut Wire,
}

impl Read for Limited<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let room = buf.len().min(self.wire.room()).min(TLS_READ_CHUNK);
        if room == 0 {
            return Err(ErrorKind::WouldBlock.into());
        }
        let n = self.tcp.read(&mut buf[..room])?;
        self.wire.count(n);
        Ok(n)
    }
}

/// A connection's bytes: the TCP stream itself, or TLS over it, metered on the wire. It never blocks (the stream is
/// non-blocking). Under TLS each read is one bounded step (at most one socket read, and the records it brought processed),
/// so a peer whose records carry no plaintext still spends the wire budget, and cannot hold a pass.
pub(crate) struct Pipe {
    tcp: TcpStream,
    tls: Option<Box<ServerConnection>>,
    wire: Wire,
}

impl Pipe {
    pub(crate) fn new(
        tcp: TcpStream,
        tls: Option<Box<ServerConnection>>,
        caps: WireCaps,
        now: Instant,
    ) -> Pipe {
        Pipe {
            tcp,
            tls,
            wire: Wire {
                caps,
                budget: caps.bytes_per_pass,
                window: now,
                bytes: 0,
                over: false,
            },
        }
    }

    /// A new pass at `now`: the wire budget is full again.
    pub(crate) fn refill(&mut self, now: Instant) {
        let wire = &mut self.wire;
        wire.budget = wire.caps.bytes_per_pass;
        if now.duration_since(wire.window) >= Duration::from_secs(1) {
            wire.window = now;
            wire.bytes = 0;
        }
    }

    /// New wire caps (the connection became a game socket); the bytes of this second still count.
    pub(crate) fn set_caps(&mut self, caps: WireCaps) {
        let wire = &mut self.wire;
        wire.caps = caps;
        wire.budget = wire.budget.min(caps.bytes_per_pass);
        if wire.bytes > caps.bytes_per_s {
            wire.over = true;
        }
    }

    /// Whether the peer sent more bytes in a second than the wire cap.
    pub(crate) fn over(&self) -> bool {
        self.wire.over
    }

    /// Whether a TLS handshake is still going on.
    pub(crate) fn is_handshaking(&self) -> bool {
        self.tls.as_ref().is_some_and(|c| c.is_handshaking())
    }

    /// Ends the connection: TLS says goodbye first (best effort), then the stream is shut.
    pub(crate) fn shut(&mut self) {
        if let Some(conn) = &mut self.tls {
            conn.send_close_notify();
            let _ = conn.write_tls(&mut self.tcp);
        }
        let _ = self.tcp.shutdown(Shutdown::Both);
    }
}

/// Sends some of what TLS has queued (a few socket writes at most): whether it all went.
fn send_queued(conn: &mut ServerConnection, tcp: &mut TcpStream) -> io::Result<bool> {
    for _ in 0..TLS_WRITES_PER_CALL {
        if !conn.wants_write() {
            return Ok(true);
        }
        match conn.write_tls(tcp) {
            Ok(0) => return Ok(false),
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(false),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(!conn.wants_write())
}

/// One TLS step: plaintext already decrypted if any, else at most one socket read and the records it completed processed,
/// then their plaintext or WouldBlock.
fn tls_read(
    conn: &mut ServerConnection,
    tcp: &mut TcpStream,
    wire: &mut Wire,
    buf: &mut [u8],
) -> io::Result<usize> {
    match conn.reader().read(buf) {
        Err(e) if e.kind() == ErrorKind::WouldBlock => {}
        got => return got,
    }
    if wire.room() == 0 {
        return Err(ErrorKind::WouldBlock.into());
    }
    match conn.read_tls(&mut Limited { tcp, wire }) {
        Ok(_) => {}
        Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
            return Err(ErrorKind::WouldBlock.into());
        }
        Err(e) => return Err(e),
    }
    let processed = conn.process_new_packets();
    // The handshake's answers, a key update's, or an error's alert.
    let sent = send_queued(conn, tcp);
    if let Err(e) = processed {
        return Err(io::Error::new(ErrorKind::InvalidData, e));
    }
    sent?;
    conn.reader().read(buf)
}

impl Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let Some(conn) = &mut self.tls else {
            let room = buf.len().min(self.wire.room());
            if room == 0 {
                return Err(ErrorKind::WouldBlock.into());
            }
            let n = self.tcp.read(&mut buf[..room])?;
            self.wire.count(n);
            return Ok(n);
        };
        tls_read(conn, &mut self.tcp, &mut self.wire, buf)
    }
}

impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let Some(conn) = &mut self.tls else {
            return self.tcp.write(buf);
        };
        send_queued(conn, &mut self.tcp)?;
        // Taken up to TLS's buffer cap, then sent as far as the socket takes it.
        let n = conn.writer().write(buf)?;
        send_queued(conn, &mut self.tcp)?;
        if n == 0 && !buf.is_empty() {
            return Err(ErrorKind::WouldBlock.into());
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        let Some(conn) = &mut self.tls else {
            return self.tcp.flush();
        };
        conn.writer().flush()?;
        if send_queued(conn, &mut self.tcp)? {
            Ok(())
        } else {
            Err(ErrorKind::WouldBlock.into())
        }
    }
}
