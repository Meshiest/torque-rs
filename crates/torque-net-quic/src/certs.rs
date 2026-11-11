//! The certificates a server presents on its identity key, for browsers: `serverCertificateHashes` (W3C WebTransport) takes
//! a certificate only if it is ECDSA P-256, valid now and valid for less than two weeks in all, so the server issues a
//! short self-signed one on the same key and re-issues it before it runs out ([`Rotation`]). The hashes browsers pin are the
//! SHA-256 of the certificate's DER: the server publishes the one it presents and the one it will present next, so a page
//! loaded just before a rotation still joins after it. A server with a certificate authority's chain on its key presents
//! that instead ([`Certificates::Chain`]) and publishes no hash.
//!
//! Natives never look at these: they pin the key's hash (`tls::PinVerifier`), the same whatever certificate carries it. The
//! short certificate has an ordinary server certificate's shape (a common name, the loopback names, key usage, server
//! authentication, explicitly not a CA): a browser's parser took a bare one, with no extensions, for no certificate at all,
//! hash or no hash.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rcgen::CertificateParams;
use rustls::pki_types::CertificateDer;

use crate::identity::{ServerKey, identity_of_certificate};

/// How long each short certificate is valid in all: under the two weeks browsers allow.
pub const SHORT_VALIDITY: Duration = Duration::from_secs(13 * 24 * 3600);
/// How often the next certificate takes over.
pub const ROTATE_EVERY: Duration = Duration::from_secs(6 * 24 * 3600);
/// How far back a certificate's `notBefore` is set, for a browser whose clock runs behind the server's.
pub const BACKDATE: Duration = Duration::from_secs(3600);
/// The most a certificate's validity may span for browsers to take its hash.
pub const BROWSER_MAX_VALIDITY: Duration = Duration::from_secs(14 * 24 * 3600);

/// A certificate's SHA-256, as `serverCertificateHashes` takes it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct CertHash(pub [u8; 32]);

impl CertHash {
    /// The hash of a certificate's DER.
    #[must_use]
    pub fn of(der: &[u8]) -> CertHash {
        let digest = ring::digest::digest(&ring::digest::SHA256, der);
        let mut out = [0; 32];
        out.copy_from_slice(digest.as_ref());
        CertHash(out)
    }

    /// Lower-case hex, as a page carries it.
    #[must_use]
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Debug for CertHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CertHash({})", self.to_hex())
    }
}

impl fmt::Display for CertHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// One short certificate on the identity key.
#[derive(Debug, Clone)]
pub struct ShortCert {
    pub der: CertificateDer<'static>,
    pub hash: CertHash,
    pub not_before: SystemTime,
    pub not_after: SystemTime,
}

/// Why a certificate could not be made or taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertError {
    /// rcgen could not sign it.
    Sign(String),
    /// A chain's leaf is not on the identity key.
    NotTheKey,
    /// A chain with no certificate.
    Empty,
}

impl fmt::Display for CertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CertError::Sign(why) => write!(f, "the certificate could not be signed: {why}"),
            CertError::NotTheKey => {
                f.write_str("the certificate chain's first certificate is not on the identity key")
            }
            CertError::Empty => f.write_str("the certificate chain is empty"),
        }
    }
}

impl std::error::Error for CertError {}

fn offset(time: SystemTime) -> Result<time::OffsetDateTime, CertError> {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map_err(|e| CertError::Sign(e.to_string()))?
        .as_secs();
    time::OffsetDateTime::from_unix_timestamp(i64::try_from(secs).unwrap_or(i64::MAX))
        .map_err(|e| CertError::Sign(e.to_string()))
}

/// A self-signed certificate on `key`, valid from `now` less [`BACKDATE`] for [`SHORT_VALIDITY`] in all.
///
/// # Errors
/// rcgen could not sign it.
pub fn short_certificate(key: &ServerKey, now: SystemTime) -> Result<ShortCert, CertError> {
    let not_before = now.checked_sub(BACKDATE).unwrap_or(UNIX_EPOCH);
    let not_after = not_before + SHORT_VALIDITY;
    // The shape of an ordinary server certificate (a name, the loopback names, the usual extensions), so a browser's parser
    // takes it as one; a browser that pins a hash checks no name, natives check the key.
    let mut params = CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()])
        .map_err(|e| CertError::Sign(e.to_string()))?;
    params.not_before = offset(not_before)?;
    params.not_after = offset(not_after)?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "torque-rs game server");
    params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    params.is_ca = rcgen::IsCa::ExplicitNoCa;
    let cert = params
        .self_signed(key.pair())
        .map_err(|e| CertError::Sign(e.to_string()))?;
    let der = cert.der().clone();
    Ok(ShortCert {
        hash: CertHash::of(&der),
        der,
        not_before,
        not_after,
    })
}

/// The server's rotating short certificates: the one presented and the one that takes over at the next rotation.
#[derive(Debug, Clone)]
pub struct Rotation {
    current: ShortCert,
    next: ShortCert,
    /// When `next` takes over.
    rotate_at: SystemTime,
}

impl Rotation {
    /// Two fresh certificates on `key` at `now`.
    ///
    /// # Errors
    /// rcgen could not sign them.
    pub fn new(key: &ServerKey, now: SystemTime) -> Result<Rotation, CertError> {
        Ok(Rotation {
            current: short_certificate(key, now)?,
            next: short_certificate(key, now)?,
            rotate_at: now + ROTATE_EVERY,
        })
    }

    /// Whether the next certificate is due to take over at `now`.
    #[must_use]
    pub fn due(&self, now: SystemTime) -> bool {
        now >= self.rotate_at
    }

    /// Makes the next certificate the current one and issues a new next; true when it did (it was due). After a long pause
    /// (the computer asleep, a clock jump) the next certificate may be too old to serve its six days: both are then issued
    /// fresh, as at the start.
    ///
    /// # Errors
    /// rcgen could not sign the new one; the rotation is unchanged then.
    pub fn rotate(&mut self, key: &ServerKey, now: SystemTime) -> Result<bool, CertError> {
        if !self.due(now) {
            return Ok(false);
        }
        if now + ROTATE_EVERY >= self.next.not_after {
            let current = short_certificate(key, now)?;
            self.next = short_certificate(key, now)?;
            self.current = current;
        } else {
            let fresh = short_certificate(key, now)?;
            self.current = std::mem::replace(&mut self.next, fresh);
        }
        self.rotate_at = now + ROTATE_EVERY;
        Ok(true)
    }

    /// The certificate presented.
    #[must_use]
    pub fn current(&self) -> &ShortCert {
        &self.current
    }

    /// The hashes to publish: the current certificate's and the next's.
    #[must_use]
    pub fn hashes(&self) -> [CertHash; 2] {
        [self.current.hash, self.next.hash]
    }
}

/// The validity a certificate's DER carries (`notBefore`, `notAfter`), read as a browser reads it; None when the bytes are
/// not a certificate as far as its validity, or a date is not a UTCTime or GeneralizedTime in UTC.
#[must_use]
pub fn validity_of_certificate(cert: &[u8]) -> Option<(SystemTime, SystemTime)> {
    use crate::identity::{DER_CONTEXT_0, DER_SEQUENCE, der_element};
    let (DER_SEQUENCE, body, _) = der_element(cert)? else {
        return None;
    };
    let (DER_SEQUENCE, mut tbs, _) = der_element(body)? else {
        return None;
    };
    if tbs.first() == Some(&DER_CONTEXT_0) {
        tbs = der_element(tbs)?.2;
    }
    // Serial, signature algorithm, issuer, then the validity.
    for _ in 0..3 {
        tbs = der_element(tbs)?.2;
    }
    let (DER_SEQUENCE, validity, _) = der_element(tbs)? else {
        return None;
    };
    let (tag, not_before, rest) = der_element(validity)?;
    let not_before = der_time(tag, not_before)?;
    let (tag, not_after, _) = der_element(rest)?;
    Some((not_before, der_time(tag, not_after)?))
}

/// A UTCTime (`YYMMDDHHMMSSZ`, tag 0x17) or GeneralizedTime (`YYYYMMDDHHMMSSZ`, tag 0x18) as a `SystemTime`.
fn der_time(tag: u8, text: &[u8]) -> Option<SystemTime> {
    let text = std::str::from_utf8(text).ok()?.strip_suffix('Z')?;
    let digits = |s: &str| -> Option<u64> {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok())?
    };
    let (year, rest) = match tag {
        0x17 => {
            let yy = digits(text.get(..2)?)?;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, text.get(2..)?)
        }
        0x18 => (digits(text.get(..4)?)?, text.get(4..)?),
        _ => return None,
    };
    if rest.len() != 10 {
        return None;
    }
    let field = |i: usize| digits(&rest[i..i + 2]);
    let (month, day, hour, minute, second) =
        (field(0)?, field(2)?, field(4)?, field(6)?, field(8)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm), valid from 1970.
    let (y, m) = if month <= 2 {
        (year as i64 - 1, month as i64 + 9)
    } else {
        (year as i64, month as i64 - 3)
    };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = u64::try_from(days).ok()? * 86_400 + hour * 3600 + minute * 60 + second;
    Some(UNIX_EPOCH + Duration::from_secs(secs))
}

/// What a server presents on its identity key.
#[derive(Debug, Clone)]
pub enum Certificates {
    /// Rotating short self-signed certificates; their hashes are published for browsers.
    Rotating(Rotation),
    /// A certificate authority's chain on the key (the first certificate's key is the identity): browsers trust it as any
    /// web certificate and need no hash. Never rotated here.
    Chain(Vec<CertificateDer<'static>>),
}

impl Certificates {
    /// The chain `chain` on `key`, checked.
    ///
    /// # Errors
    /// The chain is empty or its first certificate is not on the identity key.
    pub fn chain(
        key: &ServerKey,
        chain: Vec<CertificateDer<'static>>,
    ) -> Result<Certificates, CertError> {
        let leaf = chain.first().ok_or(CertError::Empty)?;
        if identity_of_certificate(leaf) != Some(key.identity()) {
            return Err(CertError::NotTheKey);
        }
        Ok(Certificates::Chain(chain))
    }

    /// The chain to present now.
    #[must_use]
    pub fn presented(&self) -> Vec<CertificateDer<'static>> {
        match self {
            Certificates::Rotating(r) => vec![r.current().der.clone()],
            Certificates::Chain(chain) => chain.clone(),
        }
    }

    /// The hashes browsers may pin: the rotation's two, none for a chain.
    #[must_use]
    pub fn hashes(&self) -> Vec<CertHash> {
        match self {
            Certificates::Rotating(r) => r.hashes().to_vec(),
            Certificates::Chain(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The span the DER itself carries, which is what a browser reads (rcgen writes whole seconds).
    fn validity_of(der: &[u8]) -> Duration {
        let (from, to) = validity_of_certificate(der).expect("a certificate with dates");
        to.duration_since(from).expect("ordered")
    }

    #[test]
    fn the_der_carries_the_dates_written_and_junk_has_none() {
        let key = ServerKey::generate().expect("a key");
        let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cert = short_certificate(&key, now).expect("a certificate");
        assert_eq!(
            validity_of_certificate(&cert.der),
            Some((cert.not_before, cert.not_after))
        );
        assert_eq!(cert.not_before, now - BACKDATE);
        // The long-lived certificate of the identity (year 4096: a GeneralizedTime) reads too, and spans centuries.
        let (from, to) = validity_of_certificate(key.certificate()).expect("dates");
        assert!(to.duration_since(from).expect("ordered") > BROWSER_MAX_VALIDITY * 1000);
        assert_eq!(validity_of_certificate(&[]), None);
        assert_eq!(validity_of_certificate(&cert.der[..40]), None);
        assert_eq!(
            der_time(0x17, b"991231235959Z"),
            Some(UNIX_EPOCH + Duration::from_secs(946_684_799))
        );
        assert_eq!(
            der_time(0x18, b"20000101000000Z"),
            Some(UNIX_EPOCH + Duration::from_secs(946_684_800))
        );
        assert_eq!(der_time(0x17, b"991331235959Z"), None);
        assert_eq!(der_time(0x17, b"9912312359Z"), None);
        assert_eq!(der_time(0x16, b"991231235959Z"), None);
    }

    #[test]
    fn a_rotation_over_sixty_days_keeps_every_certificate_short_and_the_live_one_published() {
        let key = ServerKey::generate().expect("a key");
        let start = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut rotation = Rotation::new(&key, start).expect("certificates");
        let mut seen = std::collections::HashSet::new();
        let mut rotations = 0;
        let step = Duration::from_secs(6 * 3600);
        let mut now = start;
        while now < start + Duration::from_secs(60 * 24 * 3600) {
            if rotation.rotate(&key, now).expect("rotates") {
                rotations += 1;
            }
            let current = rotation.current();
            assert!(
                current.not_before <= now && now < current.not_after,
                "valid at {now:?}"
            );
            assert!(
                current
                    .not_after
                    .duration_since(current.not_before)
                    .expect("ordered")
                    < BROWSER_MAX_VALIDITY
            );
            assert_eq!(validity_of(&current.der), SHORT_VALIDITY);
            assert!(rotation.hashes().contains(&current.hash));
            assert_eq!(
                identity_of_certificate(&current.der),
                Some(key.identity()),
                "the same key"
            );
            // The hashes a page took up to six days ago still name the certificate presented now.
            seen.insert(current.hash);
            now += step;
        }
        assert_eq!(rotations, 9, "every six days over sixty");
        assert_eq!(seen.len(), 10, "ten distinct certificates were presented");
        let hashes = Certificates::Rotating(rotation.clone()).hashes();
        assert_eq!(hashes.len(), 2);
        assert_ne!(hashes[0], hashes[1]);
        assert_eq!(hashes[0].to_hex().len(), 64);
    }

    #[test]
    fn a_page_loaded_just_before_a_rotation_still_has_the_certificate_presented_after_it() {
        let key = ServerKey::generate().expect("a key");
        let start = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut rotation = Rotation::new(&key, start).expect("certificates");
        let page = rotation.hashes();
        let later = start + ROTATE_EVERY + Duration::from_secs(1);
        assert!(rotation.rotate(&key, later).expect("rotates"));
        assert!(page.contains(&rotation.current().hash));
        assert!(!rotation.rotate(&key, later).expect("not due"));
        // A page older than one rotation is out of date.
        let much_later = later + ROTATE_EVERY;
        assert!(rotation.rotate(&key, much_later).expect("rotates"));
        assert!(!page.contains(&rotation.current().hash));
    }

    #[test]
    fn a_long_pause_issues_both_certificates_fresh_so_the_one_presented_is_valid() {
        let key = ServerKey::generate().expect("a key");
        let start = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut rotation = Rotation::new(&key, start).expect("certificates");
        let old_next = rotation.hashes()[1];
        // Twenty days asleep: the next certificate made at the start has run out.
        let later = start + Duration::from_secs(20 * 24 * 3600);
        assert!(rotation.rotate(&key, later).expect("rotates"));
        let current = rotation.current();
        assert!(
            current.not_before <= later && later < current.not_after,
            "valid now"
        );
        assert!(
            later + ROTATE_EVERY < current.not_after,
            "and for its six days"
        );
        assert_ne!(current.hash, old_next, "not the stale next");
        assert!(later + ROTATE_EVERY < rotation.next.not_after);
        // A pause shorter than a week still promotes the next, which is good for the six days it serves.
        let mut rotation = Rotation::new(&key, start).expect("certificates");
        let next = rotation.hashes()[1];
        let soon = start + ROTATE_EVERY + Duration::from_secs(12 * 3600);
        assert!(rotation.rotate(&key, soon).expect("rotates"));
        assert_eq!(rotation.current().hash, next);
    }

    #[test]
    fn a_chain_must_be_on_the_identity_key() {
        let key = ServerKey::generate().expect("a key");
        let other = ServerKey::generate().expect("another key");
        assert_eq!(
            Certificates::chain(&key, Vec::new()).err(),
            Some(CertError::Empty)
        );
        assert_eq!(
            Certificates::chain(&key, vec![other.certificate().clone()]).err(),
            Some(CertError::NotTheKey)
        );
        let chain =
            Certificates::chain(&key, vec![key.certificate().clone()]).expect("the key's own");
        assert!(chain.hashes().is_empty());
        assert_eq!(chain.presented().len(), 1);
    }
}
