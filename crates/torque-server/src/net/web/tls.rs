//! TLS for the web transport (`wss` and `https`): a certificate chain and its key read from PEM files, rustls with the ring
//! provider, and the byte stream a connection reads and writes, plain or under TLS.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

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

    pub(crate) fn wrap(&self, tcp: TcpStream) -> io::Result<Pipe> {
        let conn = ServerConnection::new(Arc::clone(&self.config))
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(Pipe::Tls(Box::new(StreamOwned::new(conn, tcp))))
    }
}

/// A connection's bytes: the TCP stream itself, or TLS over it. Both never block (the stream is non-blocking).
pub(crate) enum Pipe {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ServerConnection, TcpStream>>),
}

impl Pipe {
    /// Whether a TLS handshake is still going on.
    pub(crate) fn is_handshaking(&self) -> bool {
        match self {
            Pipe::Plain(_) => false,
            Pipe::Tls(s) => s.conn.is_handshaking(),
        }
    }

    /// Ends the connection: TLS says goodbye first (best effort), then the stream is shut.
    pub(crate) fn shut(&mut self) {
        let tcp = match self {
            Pipe::Plain(tcp) => tcp,
            Pipe::Tls(s) => {
                s.conn.send_close_notify();
                let _ = s.conn.write_tls(&mut s.sock);
                &mut s.sock
            }
        };
        let _ = tcp.shutdown(Shutdown::Both);
    }
}

impl Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Pipe::Plain(tcp) => tcp.read(buf),
            Pipe::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Pipe::Plain(tcp) => tcp.write(buf),
            Pipe::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Pipe::Plain(tcp) => tcp.flush(),
            Pipe::Tls(s) => s.flush(),
        }
    }
}
