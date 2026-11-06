//! The ICE servers a room's players are given: STUN URLs (a peer learns the address its NAT gives it), and TURN URLs with
//! short-lived credentials (a peer whose NAT lets nothing in relays through the TURN server).
//!
//! **TURN credentials** follow the TURN REST API scheme that coturn takes with `use-auth-secret` and `static-auth-secret`:
//! the user name is `<expiry>:<id>` (the expiry in Unix seconds) and the password is the Base64 of HMAC-SHA1 over the user
//! name, keyed with the secret the service and the TURN server share. The TURN server checks the expiry itself, so the
//! service keeps no list of credentials. A relayed connection ends when its credential expires (the TURN server refuses
//! the allocation's refresh), so the lifetime ([`Ice::ttl`]) should cover a game session. The service hands credentials
//! only to a socket that hosts or has joined a room; the TURN server must keep its own quotas (allocations per user and
//! in all, bandwidth), since anyone can open a room.

use std::fmt::Write as _;
use std::time::Duration;

use ring::hmac;
use torque_server::net::web::http::base64;

/// The longest URL taken.
pub const MAX_URL: usize = 256;
/// The most URLs of each kind.
pub const MAX_URLS: usize = 8;
/// The longest secret taken.
pub const MAX_SECRET: usize = 1024;

/// What a room's players are given for ICE.
#[derive(Clone, Default)]
pub struct Ice {
    /// `stun:` / `stuns:` URLs.
    pub stun: Vec<String>,
    /// `turn:` / `turns:` URLs (all under one credential).
    pub turn: Vec<String>,
    /// The secret shared with the TURN server (none: no TURN URLs are given).
    pub secret: Option<Vec<u8>>,
    /// How long a credential lasts.
    pub ttl: Duration,
}

impl std::fmt::Debug for Ice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ice")
            .field("stun", &self.stun)
            .field("turn", &self.turn)
            .field("secret", &self.secret.as_ref().map(|_| "(set)"))
            .field("ttl", &self.ttl)
            .finish()
    }
}

/// The default lifetime of a TURN credential: long enough for a game session.
pub const DEFAULT_TTL: Duration = Duration::from_secs(4 * 3600);
/// The longest lifetime of a TURN credential (a room's longest life).
pub const MAX_TTL: Duration = Duration::from_secs(48 * 3600);

/// Checks a STUN or TURN URL: its scheme is one of `schemes`, and it is printable ASCII with no quote, backslash, space or
/// control character (it goes into JSON as it is), at most [`MAX_URL`] bytes.
pub fn check_url(url: &str, schemes: &[&str]) -> Result<(), String> {
    if url.len() > MAX_URL {
        return Err(format!("a URL longer than {MAX_URL} bytes"));
    }
    if !schemes.iter().any(|s| {
        url.strip_prefix(s)
            .and_then(|rest| rest.strip_prefix(':'))
            .is_some_and(|rest| !rest.is_empty())
    }) {
        return Err(format!(
            "{url:?} does not start with {}",
            schemes.join(": or ") + ":"
        ));
    }
    if !url
        .bytes()
        .all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\')
    {
        return Err(format!("{url:?} has a character a URL here may not"));
    }
    Ok(())
}

/// Reads a TURN secret from a file's text: trimmed of white space at the ends, not empty, at most [`MAX_SECRET`] bytes.
pub fn read_secret(text: &[u8]) -> Result<Vec<u8>, String> {
    let start = text.iter().position(|b| !b.is_ascii_whitespace());
    let end = text.iter().rposition(|b| !b.is_ascii_whitespace());
    let secret = match (start, end) {
        (Some(s), Some(e)) => text.get(s..=e).unwrap_or_default(),
        _ => return Err("the TURN secret file is empty".into()),
    };
    if secret.len() > MAX_SECRET {
        return Err(format!("the TURN secret is longer than {MAX_SECRET} bytes"));
    }
    Ok(secret.to_vec())
}

/// Reads the TURN secret from the file at `path` ([`read_secret`]'s rules): a regular file, read only up to a little past
/// [`MAX_SECRET`] bytes (a longer file is refused, not read whole), and on Unix not readable by other users.
pub fn read_secret_file(path: &std::path::Path) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    let shown = path.display();
    let file =
        std::fs::File::open(path).map_err(|e| format!("the TURN secret file {shown}: {e}"))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("the TURN secret file {shown}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("the TURN secret file {shown} is not a file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if meta.permissions().mode() & 0o007 != 0 {
            return Err(format!(
                "the TURN secret file {shown} is open to other users (chmod o-rwx it)"
            ));
        }
    }
    let cap = MAX_SECRET + SECRET_SPACE;
    let mut text = Vec::new();
    file.take(cap as u64 + 1)
        .read_to_end(&mut text)
        .map_err(|e| format!("the TURN secret file {shown}: {e}"))?;
    if text.len() > cap {
        return Err(format!(
            "the TURN secret file {shown} is longer than {cap} bytes"
        ));
    }
    read_secret(&text)
}

/// The white space a secret file may have around its secret.
pub const SECRET_SPACE: usize = 256;

/// The TURN REST API credential for `id` expiring at `expiry` (Unix seconds): the user name and the password.
pub fn credential(secret: &[u8], expiry: u64, id: &str) -> (String, String) {
    let username = format!("{expiry}:{id}");
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let tag = hmac::sign(&key, username.as_bytes());
    (username, base64(tag.as_ref()))
}

impl Ice {
    /// Checks the URLs and the setup (TURN URLs need a secret and a lifetime).
    pub fn check(&self) -> Result<(), String> {
        if self.stun.len() > MAX_URLS || self.turn.len() > MAX_URLS {
            return Err(format!("at most {MAX_URLS} URLs of each kind"));
        }
        for url in &self.stun {
            check_url(url, &["stun", "stuns"])?;
        }
        for url in &self.turn {
            check_url(url, &["turn", "turns"])?;
        }
        if !self.turn.is_empty() && self.secret.is_none() {
            return Err("TURN URLs need the secret shared with the TURN server".into());
        }
        if !self.turn.is_empty() && self.ttl < Duration::from_secs(60) {
            return Err("a TURN credential must last at least a minute".into());
        }
        if self.ttl > MAX_TTL {
            return Err(format!(
                "a TURN credential lasts at most {} hours",
                MAX_TTL.as_secs() / 3600
            ));
        }
        Ok(())
    }

    /// The ICE servers for `RTCPeerConnection` (a JSON array), with a TURN credential for `id` made at `now` (Unix
    /// seconds).
    pub fn servers(&self, now: u64, id: &str) -> String {
        let urls = |list: &[String]| {
            list.iter()
                .map(|u| format!("\"{u}\""))
                .collect::<Vec<_>>()
                .join(",")
        };
        let mut out = String::from("[");
        if !self.stun.is_empty() {
            let _ = write!(out, "{{\"urls\":[{}]}}", urls(&self.stun));
        }
        if let Some(secret) = self.secret.as_deref().filter(|_| !self.turn.is_empty()) {
            let (username, password) =
                credential(secret, now.saturating_add(self.ttl.as_secs()), id);
            if out.len() > 1 {
                out.push(',');
            }
            // The user name is digits, a colon and the id (letters and digits); the password is Base64.
            let _ = write!(
                out,
                "{{\"urls\":[{}],\"username\":\"{username}\",\"credential\":\"{password}\"}}",
                urls(&self.turn)
            );
        }
        out.push(']');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_match_a_reference_computed_elsewhere() {
        // HMAC-SHA1 and Base64 computed with Python's hmac, hashlib and base64 modules.
        assert_eq!(
            credential(b"north-wind-secret", 1_700_003_600, "42"),
            (
                "1700003600:42".to_string(),
                "lX5Gk8AKmXn0FUqv/skP+33Jzio=".to_string()
            )
        );
        // A key longer than SHA-1's block is hashed first.
        assert_eq!(
            credential(&[b'k'; 100], 1, "1").1,
            "YFoYXfmHpDY8fiUoghnjOJDLZp8="
        );
    }

    #[test]
    fn servers_carry_stun_and_a_turn_credential_that_expires_after_the_ttl() {
        let ice = Ice {
            stun: vec!["stun:stun.example:3478".into()],
            turn: vec![
                "turn:turn.example:3478?transport=udp".into(),
                "turns:turn.example:5349".into(),
            ],
            secret: Some(b"north-wind-secret".to_vec()),
            ttl: Duration::from_secs(3600),
        };
        ice.check().unwrap();
        assert_eq!(
            ice.servers(1_700_000_000, "42"),
            "[{\"urls\":[\"stun:stun.example:3478\"]},{\"urls\":[\"turn:turn.example:3478?transport=udp\",\
             \"turns:turn.example:5349\"],\"username\":\"1700003600:42\",\"credential\":\"lX5Gk8AKmXn0FUqv/skP+33Jzio=\"}]"
        );
        assert_eq!(Ice::default().servers(5, "1"), "[]");
        let stun_only = Ice {
            stun: vec!["stun:192.0.2.1:3478".into()],
            ..Ice::default()
        };
        assert_eq!(
            stun_only.servers(5, "1"),
            "[{\"urls\":[\"stun:192.0.2.1:3478\"]}]"
        );
        // A secret with no TURN URL gives nothing away.
        let secret_only = Ice {
            secret: Some(b"x".to_vec()),
            ..Ice::default()
        };
        assert_eq!(secret_only.servers(5, "1"), "[]");
    }

    #[test]
    fn bad_setups_are_refused() {
        for (url, schemes) in [
            ("http://x", &["stun", "stuns"][..]),
            ("stun:", &["stun"][..]),
            ("stun:a\"b", &["stun"][..]),
            ("stun:a b", &["stun"][..]),
            ("stun:a\\b", &["stun"][..]),
            ("turn:x", &["stun"][..]),
        ] {
            assert!(check_url(url, schemes).is_err(), "{url}");
        }
        assert!(check_url(&format!("stun:{}", "a".repeat(MAX_URL)), &["stun"]).is_err());
        let no_secret = Ice {
            turn: vec!["turn:x".into()],
            ttl: DEFAULT_TTL,
            ..Ice::default()
        };
        assert!(no_secret.check().is_err());
        let short = Ice {
            turn: vec!["turn:x".into()],
            secret: Some(b"s".to_vec()),
            ttl: Duration::from_secs(5),
            ..Ice::default()
        };
        assert!(short.check().is_err());
        let long = Ice {
            ttl: MAX_TTL + Duration::from_secs(1),
            ..short.clone()
        };
        assert!(long.check().is_err());
        let longest = Ice {
            ttl: MAX_TTL,
            ..short
        };
        longest.check().unwrap();
        // An expiry past the clock's end does not wrap to the past.
        assert!(
            longest
                .servers(u64::MAX - 5, "1")
                .contains(&format!("\"{}:1\"", u64::MAX))
        );
        assert_eq!(read_secret(b"  s3cret\r\n").unwrap(), b"s3cret");
        assert!(read_secret(b" \n\t").is_err());
        assert!(read_secret(&[b'a'; MAX_SECRET + 1]).is_err());
        // The secret never shows in a debug print.
        let shown = format!(
            "{:?}",
            Ice {
                secret: Some(b"hidden".to_vec()),
                ..Ice::default()
            }
        );
        assert!(!shown.contains("hidden"));
    }
}
