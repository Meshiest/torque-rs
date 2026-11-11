//! The TLS and QUIC configs both ends run with, and the client's verifier: it pins the server's key hash and uses no web PKI.

use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use quinn_proto::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn_proto::{
    ClientConfig, EndpointConfig, IdleTimeout, ServerConfig, TransportConfig, VarInt,
};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use torque_net::engine::ServerIdentity;

use crate::clock::Clock;
use crate::identity::{ServerKey, identity_of_certificate};

/// The application protocol natives speak: the engine's datagrams straight on QUIC DATAGRAM frames. Provisional: the
/// HTTP/3 WebTransport session that browsers need replaces it with `h3` (one session protocol for both), without changing
/// the engine bytes.
pub const ALPN: &[u8] = b"torque-engine-raw-1";

/// The largest engine datagram a QUIC link reports, whatever the path allows: the engine's own size on every other link.
pub const QUIC_MAX_DATAGRAM: usize = 1_200;

/// The caps and timeouts of an endpoint's connections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuicLimits {
    /// The most established connections a server holds at once; a validated handshake past it is refused.
    pub max_connections: usize,
    /// The most established connections a server holds from one address (checked again when a connection's path moves).
    pub connections_per_ip: usize,
    /// The most handshakes in progress (not yet established) the server holds; an unvalidated Initial past it is answered
    /// with a Retry, and a validated one evicts the oldest handshake in progress.
    pub handshakes_in_progress: usize,
    /// A handshake not established within this long is dropped (apart from the idle timeout of established connections).
    pub handshake_timeout: Duration,
    /// The most validated handshakes in progress from one address; a validated Initial past it is refused. (Unvalidated
    /// ones hold no share: their address may be forged.)
    pub handshakes_in_progress_per_ip: usize,
    /// The most validated handshakes in progress from one network (an IPv4 /24, an IPv6 /48); a validated Initial past
    /// it is refused.
    pub handshakes_in_progress_per_network: usize,
    /// New handshakes a second (all addresses) above which the server answers unvalidated ones with Retry first.
    pub handshakes_per_second: u32,
    /// New handshakes a second from one network (an IPv4 /24, an IPv6 /48) above which it answers with Retry.
    pub handshakes_per_network_per_second: u32,
    /// The most application datagrams a server holds for the host from one connection before it reads them; a datagram past
    /// it is dropped, and only that connection's.
    pub inbox_per_connection: usize,
    /// The most bytes of application datagrams a server holds for the host from one connection. A datagram larger than
    /// [`QUIC_MAX_DATAGRAM`] is dropped on receipt whatever the room.
    pub inbox_bytes_per_connection: usize,
    /// The bytes of application datagrams a connection buffers for the host (quinn-proto drops the oldest past it).
    pub datagram_receive_buffer: usize,
    /// The bytes of application datagrams a connection holds for sending (a send past it is refused: "no room").
    pub datagram_send_buffer: usize,
    /// A connection with no packet either way for this long is closed.
    pub idle_timeout: Duration,
    /// A client pings after this long without a packet, so a quiet connection is kept.
    pub keep_alive: Duration,
}

impl Default for QuicLimits {
    fn default() -> QuicLimits {
        QuicLimits {
            max_connections: 256,
            connections_per_ip: 16,
            handshakes_in_progress: 64,
            handshake_timeout: Duration::from_secs(5),
            handshakes_in_progress_per_ip: 4,
            handshakes_in_progress_per_network: 16,
            handshakes_per_second: 32,
            handshakes_per_network_per_second: 8,
            inbox_per_connection: 256,
            inbox_bytes_per_connection: 256 * QUIC_MAX_DATAGRAM,
            datagram_receive_buffer: 64 << 10,
            datagram_send_buffer: 64 << 10,
            idle_timeout: Duration::from_secs(30),
            keep_alive: Duration::from_secs(5),
        }
    }
}

/// The transport config of every connection: datagrams on, no streams (this crate carries the engine's datagrams only),
/// bounded datagram buffers, one datagram per transmit.
#[must_use]
pub fn transport_config(limits: &QuicLimits, client: bool) -> TransportConfig {
    let mut transport = TransportConfig::default();
    transport
        .max_concurrent_bidi_streams(VarInt::from_u32(0))
        .max_concurrent_uni_streams(VarInt::from_u32(0))
        .datagram_receive_buffer_size(Some(limits.datagram_receive_buffer))
        .datagram_send_buffer_size(limits.datagram_send_buffer)
        .max_idle_timeout(IdleTimeout::try_from(limits.idle_timeout).ok())
        .initial_mtu(QUIC_MAX_DATAGRAM as u16)
        .min_mtu(QUIC_MAX_DATAGRAM as u16)
        .enable_segmentation_offload(false);
    if client {
        transport.keep_alive_interval(Some(limits.keep_alive));
    }
    transport
}

/// The endpoint config: the fixed bit always set (a datagram without it is not QUIC and can share the socket), and the
/// RNG seeded for the deterministic tests (none: the system's random source).
#[must_use]
pub fn endpoint_config(seed: Option<[u8; 32]>) -> EndpointConfig {
    let mut config = EndpointConfig::default();
    config.grease_quic_bit(false).rng_seed(seed);
    config
}

/// Why a TLS config could not be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsError(pub String);

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the TLS config: {}", self.0)
    }
}

impl std::error::Error for TlsError {}

/// The server's QUIC config on `key`: TLS 1.3 only, the self-signed certificate, no client certificates, no early data,
/// migration on, and the handshake tokens stamped by `clock`.
///
/// # Errors
/// rustls refused the key or the certificate.
pub fn server_config(
    key: &ServerKey,
    limits: &QuicLimits,
    clock: &dyn Clock,
) -> Result<ServerConfig, TlsError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| TlsError(e.to_string()))?
        .with_no_client_auth()
        .with_single_cert(vec![key.certificate().clone()], key.private_key())
        .map_err(|e| TlsError(e.to_string()))?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    // No 0-RTT: a connect carries a ticket and a password, and early data is replayable.
    tls.max_early_data_size = 0;
    let crypto = QuicServerConfig::try_from(tls).map_err(|e| TlsError(e.to_string()))?;
    let mut config = ServerConfig::with_crypto(Arc::new(crypto));
    config
        .transport_config(Arc::new(transport_config(limits, false)))
        .migration(true)
        .time_source(clock.time_source())
        // Handshakes are decided as they arrive; nothing waits in the endpoint.
        .max_incoming(16)
        .incoming_buffer_size_total(256 << 10);
    Ok(config)
}

/// The client's QUIC config: TLS 1.3 only, the server's key checked by `verifier`, no early data.
///
/// # Errors
/// rustls has no TLS 1.3 suite for QUIC (never with the ring provider).
pub fn client_config(
    verifier: Arc<PinVerifier>,
    limits: &QuicLimits,
) -> Result<ClientConfig, TlsError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| TlsError(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    tls.enable_early_data = false;
    let crypto = QuicClientConfig::try_from(tls).map_err(|e| TlsError(e.to_string()))?;
    let mut config = ClientConfig::new(Arc::new(crypto));
    config.transport_config(Arc::new(transport_config(limits, true)));
    Ok(config)
}

/// Where the identity a client expects came from, for the refusal's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expected {
    /// Given with the address (a listing, an invite, a typed hash).
    Given,
    /// Remembered in the known-servers file from an earlier connection.
    Remembered,
}

#[derive(Debug, Default)]
struct Seen {
    identity: Option<ServerIdentity>,
    refusal: Option<String>,
}

/// The client's certificate check: the server's key must hash to the identity expected, if one is; the signature over the
/// handshake is verified against that key either way (proof of possession). No certificate authority, no names, no dates:
/// the key is the identity.
#[derive(Debug)]
pub struct PinVerifier {
    server: SocketAddr,
    expected: Option<(ServerIdentity, Expected)>,
    algorithms: WebPkiSupportedAlgorithms,
    seen: Mutex<Seen>,
}

impl PinVerifier {
    /// A verifier for the server at `server`, expecting `expected` (none: trust the key presented and report it).
    #[must_use]
    pub fn new(server: SocketAddr, expected: Option<(ServerIdentity, Expected)>) -> PinVerifier {
        PinVerifier {
            server,
            expected,
            algorithms: rustls::crypto::ring::default_provider().signature_verification_algorithms,
            seen: Mutex::new(Seen::default()),
        }
    }

    /// The identity the server presented and the check accepted, once the handshake reached the certificate.
    #[must_use]
    pub fn seen(&self) -> Option<ServerIdentity> {
        self.seen.lock().ok().and_then(|s| s.identity)
    }

    /// Why the certificate was refused, if it was.
    #[must_use]
    pub fn refusal(&self) -> Option<String> {
        self.seen.lock().ok().and_then(|s| s.refusal.clone())
    }

    /// The identity expected, if one is.
    #[must_use]
    pub fn expected(&self) -> Option<ServerIdentity> {
        self.expected.map(|(id, _)| id)
    }

    fn refuse(&self, why: String) -> rustls::Error {
        if let Ok(mut seen) = self.seen.lock() {
            seen.refusal = Some(why.clone());
        }
        rustls::Error::General(why)
    }
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let Some(got) = identity_of_certificate(end_entity) else {
            return Err(self.refuse(format!(
                "the server at {} presented a certificate that is not one",
                self.server
            )));
        };
        match self.expected {
            Some((want, _)) if want == got => {}
            Some((want, Expected::Given)) => {
                return Err(self.refuse(format!(
                    "the server at {} is {got}, not the identity given, {want}",
                    self.server
                )));
            }
            Some((want, Expected::Remembered)) => {
                return Err(self.refuse(format!(
                    "the server at {} is {got}, not the identity remembered for it, {want}; if its key really changed, remove its line from the known servers file",
                    self.server
                )));
            }
            None => {}
        }
        if let Ok(mut seen) = self.seen.lock() {
            seen.identity = Some(got);
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}
