//! The server's identity: one long-lived ECDSA P-256 key, kept as PKCS#8 PEM in the server's data folder
//! ([`IDENTITY_FILE`], made on first start), and the identity itself, the SHA-256 of the key's SubjectPublicKeyInfo
//! ([`ServerIdentity`], written `sha256:<hex>`). The server presents a self-signed certificate on that key; a client checks the
//! certificate's key against the identity it was given or remembers, never a certificate authority.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use torque_net::engine::ServerIdentity;

/// The identity key's file name in the server's data folder.
pub const IDENTITY_FILE: &str = "identity.key";

/// The most bytes an identity file may hold (a PKCS#8 PEM of a P-256 key is a few hundred).
pub const MAX_IDENTITY_FILE_BYTES: u64 = 16 << 10;

/// Why an identity key could not be had.
#[derive(Debug)]
pub enum IdentityError {
    /// Reading or writing the key file failed.
    Io(PathBuf, io::Error),
    /// The file is larger than a key file can be.
    TooLarge(PathBuf),
    /// The file is not a PKCS#8 PEM of an ECDSA P-256 key.
    NotAKey(PathBuf, String),
    /// The key could not be made or its certificate signed.
    Crypto(String),
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdentityError::Io(path, e) => write!(f, "the identity key {}: {e}", path.display()),
            IdentityError::TooLarge(path) => write!(
                f,
                "the identity key {} is larger than {MAX_IDENTITY_FILE_BYTES} bytes",
                path.display()
            ),
            IdentityError::NotAKey(path, why) => write!(
                f,
                "the identity key {} is not a PKCS#8 PEM of an ECDSA P-256 key: {why}",
                path.display()
            ),
            IdentityError::Crypto(why) => write!(f, "the identity key: {why}"),
        }
    }
}

impl std::error::Error for IdentityError {}

/// The server's key, its identity and the self-signed certificate it presents.
pub struct ServerKey {
    pair: KeyPair,
    cert: CertificateDer<'static>,
    identity: ServerIdentity,
}

impl fmt::Debug for ServerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerKey")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl ServerKey {
    /// A new key from the system's random source.
    ///
    /// # Errors
    /// The key could not be made or its certificate signed.
    pub fn generate() -> Result<ServerKey, IdentityError> {
        let pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .map_err(|e| IdentityError::Crypto(e.to_string()))?;
        ServerKey::from_pair(pair)
    }

    /// The key in a PKCS#8 PEM text, as [`ServerKey::to_pkcs8_pem`] writes it.
    ///
    /// # Errors
    /// Not a PKCS#8 PEM of an ECDSA P-256 key.
    pub fn from_pkcs8_pem(text: &str) -> Result<ServerKey, IdentityError> {
        let pair = KeyPair::from_pkcs8_pem_and_sign_algo(text, &PKCS_ECDSA_P256_SHA256)
            .map_err(|e| IdentityError::NotAKey(PathBuf::new(), e.to_string()))?;
        ServerKey::from_pair(pair)
    }

    fn from_pair(pair: KeyPair) -> Result<ServerKey, IdentityError> {
        // No names: a client checks the key, never the subject. Long-lived; the browsers' short certificates come later.
        let params = CertificateParams::new(Vec::<String>::new())
            .map_err(|e| IdentityError::Crypto(e.to_string()))?;
        let cert = params
            .self_signed(&pair)
            .map_err(|e| IdentityError::Crypto(e.to_string()))?;
        let cert = cert.der().clone();
        let identity = identity_of_certificate(&cert).ok_or_else(|| {
            IdentityError::Crypto("the certificate's SubjectPublicKeyInfo cannot be found".into())
        })?;
        Ok(ServerKey {
            pair,
            cert,
            identity,
        })
    }

    /// The key at `path`, or a new one written there (0600 where the platform has modes) when there is none.
    ///
    /// # Errors
    /// The file cannot be read or written, is too large, or is not a key.
    pub fn load_or_make(path: &Path) -> Result<ServerKey, IdentityError> {
        match fs::metadata(path) {
            Ok(meta) if meta.len() > MAX_IDENTITY_FILE_BYTES => {
                return Err(IdentityError::TooLarge(path.to_path_buf()));
            }
            Ok(_) => {
                let text = fs::read_to_string(path)
                    .map_err(|e| IdentityError::Io(path.to_path_buf(), e))?;
                return ServerKey::from_pkcs8_pem(&text).map_err(|e| match e {
                    IdentityError::NotAKey(_, why) => {
                        IdentityError::NotAKey(path.to_path_buf(), why)
                    }
                    other => other,
                });
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(IdentityError::Io(path.to_path_buf(), e)),
        }
        let key = ServerKey::generate()?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| IdentityError::Io(path.to_path_buf(), e))?;
        }
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let written = options.open(path).and_then(|mut file| {
            file.write_all(key.to_pkcs8_pem().as_bytes())?;
            file.sync_all()
        });
        match written {
            Ok(()) => Ok(key),
            // Another server of this folder made the key first: take that one.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => ServerKey::load_or_make(path),
            Err(e) => Err(IdentityError::Io(path.to_path_buf(), e)),
        }
    }

    /// The key as PKCS#8 PEM.
    #[must_use]
    pub fn to_pkcs8_pem(&self) -> String {
        self.pair.serialize_pem()
    }

    /// The identity: the SHA-256 of the key's SubjectPublicKeyInfo.
    #[must_use]
    pub fn identity(&self) -> ServerIdentity {
        self.identity
    }

    /// The self-signed certificate on the key.
    #[must_use]
    pub fn certificate(&self) -> &CertificateDer<'static> {
        &self.cert
    }

    /// The key pair itself, for signing a certificate on it.
    #[must_use]
    pub(crate) fn pair(&self) -> &KeyPair {
        &self.pair
    }

    /// The private key, for the TLS config.
    #[must_use]
    pub fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.pair.serialize_der()))
    }
}

/// The identity a certificate carries: the SHA-256 of its SubjectPublicKeyInfo. None when `cert` is not an X.509
/// certificate as far as the SubjectPublicKeyInfo.
#[must_use]
pub fn identity_of_certificate(cert: &[u8]) -> Option<ServerIdentity> {
    let spki = spki_of_certificate(cert)?;
    Some(identity_of_spki(spki))
}

/// The identity of a SubjectPublicKeyInfo's DER.
#[must_use]
pub fn identity_of_spki(spki: &[u8]) -> ServerIdentity {
    let digest = ring::digest::digest(&ring::digest::SHA256, spki);
    let mut out = [0; 32];
    out.copy_from_slice(digest.as_ref());
    ServerIdentity(out)
}

pub(crate) const DER_SEQUENCE: u8 = 0x30;
pub(crate) const DER_CONTEXT_0: u8 = 0xa0;

/// One DER element at the start of `input`: its tag, its value and what follows it. None when the bytes are not one.
pub(crate) fn der_element(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = input.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        // Long form: `first & 0x7f` length bytes, at most 4 (a certificate is far smaller than 4 GiB); 0x80 (indefinite) is
        // not DER.
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 || rest.len() < count {
            return None;
        }
        let (bytes, rest) = rest.split_at(count);
        let len = bytes.iter().fold(0usize, |n, &b| n << 8 | usize::from(b));
        (len, rest)
    };
    if rest.len() < len {
        return None;
    }
    let (value, after) = rest.split_at(len);
    Some((tag, value, after))
}

/// The SubjectPublicKeyInfo element (tag, length and value) of an X.509 certificate's DER.
fn spki_of_certificate(cert: &[u8]) -> Option<&[u8]> {
    let (DER_SEQUENCE, body, _) = der_element(cert)? else {
        return None;
    };
    let (DER_SEQUENCE, mut tbs, _) = der_element(body)? else {
        return None;
    };
    // TBSCertificate: [0] version (optional), serial, signature algorithm, issuer, validity, subject, then the key.
    if tbs.first() == Some(&DER_CONTEXT_0) {
        tbs = der_element(tbs)?.2;
    }
    for _ in 0..5 {
        tbs = der_element(tbs)?.2;
    }
    let (DER_SEQUENCE, _, after) = der_element(tbs)? else {
        return None;
    };
    Some(&tbs[..tbs.len() - after.len()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::PublicKeyData;

    #[test]
    fn the_identity_is_the_hash_of_the_keys_spki_and_survives_the_pem_round_trip() {
        let key = ServerKey::generate().expect("a key");
        let spki = key.pair.subject_public_key_info();
        assert_eq!(key.identity(), identity_of_spki(&spki));
        assert_eq!(
            identity_of_certificate(key.certificate()),
            Some(key.identity())
        );
        let again = ServerKey::from_pkcs8_pem(&key.to_pkcs8_pem()).expect("the same key");
        assert_eq!(again.identity(), key.identity());
        assert_ne!(
            ServerKey::generate().expect("another key").identity(),
            key.identity()
        );
    }

    #[test]
    fn a_certificate_that_is_not_one_has_no_identity_and_never_panics() {
        let key = ServerKey::generate().expect("a key");
        let der = key.certificate().as_ref();
        assert!(identity_of_certificate(&[]).is_none());
        assert!(identity_of_certificate(&[0x30, 0x80]).is_none());
        assert!(identity_of_certificate(&[0x30, 0x84, 0xff, 0xff, 0xff, 0xff]).is_none());
        for cut in [1, 5, 17, der.len() / 2, der.len() - 1] {
            assert!(
                identity_of_certificate(&der[..cut]).is_none(),
                "cut at {cut}"
            );
        }
        let mut flipped = der.to_vec();
        flipped[0] ^= 0xff;
        assert!(identity_of_certificate(&flipped).is_none());
    }

    #[test]
    fn load_or_make_writes_a_key_once_and_reads_it_back() {
        let dir = std::env::temp_dir().join(format!(
            "torque-net-quic-identity-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let path = dir.join("deeper").join(IDENTITY_FILE);
        let first = ServerKey::load_or_make(&path).expect("a key is made");
        let second = ServerKey::load_or_make(&path).expect("the key is read");
        assert_eq!(first.identity(), second.identity());
        fs::write(&path, "not a key").expect("overwritten");
        let error = ServerKey::load_or_make(&path).expect_err("not a key");
        assert!(
            matches!(error, IdentityError::NotAKey(ref p, _) if *p == path),
            "{error}"
        );
        fs::write(&path, vec![b'-'; MAX_IDENTITY_FILE_BYTES as usize + 1]).expect("overwritten");
        assert!(matches!(
            ServerKey::load_or_make(&path),
            Err(IdentityError::TooLarge(_))
        ));
        fs::remove_dir_all(&dir).expect("removed");
    }
}
