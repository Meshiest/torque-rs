//! The address a script connects to: TGE's `Net::openConnectTo` text (`[ip:]host[:port]`), and for HTTPObject an `http://`
//! or `https://` in front.

use std::net::Ipv4Addr;

/// What port an address without one gets (TGE's `defaultPort`).
pub const DEFAULT_PORT: u16 = 28_000;

/// The longest address text read (TGE copies it into a 256-byte buffer).
const MAX_ADDRESS: usize = 255;

/// Where a connect goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) host: Host,
    pub(crate) port: u16,
    /// Whether the connection is under TLS (`https://`).
    pub(crate) tls: bool,
}

/// A target's host: an IPv4 address in numbers connects at once; a name is looked up first (and reported by
/// `onDNSResolved` or `onDNSFailed`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Host {
    Ip(Ipv4Addr),
    Name(String),
    /// A name with a space, a control byte, DEL or a byte past ASCII: no lookup, `onDNSFailed` (it would otherwise reach
    /// the `Host` header of a request as it is).
    Unusable,
}

/// Whether `name` may be looked up and written in a `Host` header.
fn usable_name(name: &str) -> bool {
    name.bytes().all(|b| b > b' ' && b < 0x7F)
}

impl Host {
    /// The name TLS checks the certificate against, and the `Host` header.
    pub(crate) fn text(&self) -> String {
        match self {
            Host::Ip(ip) => ip.to_string(),
            Host::Name(name) => name.clone(),
            Host::Unusable => String::new(),
        }
    }
}

/// TGE's `dAtoi` of a port, kept to 16 bits as `htons` keeps it: leading blanks, a sign, the digits.
fn port_number(text: &str) -> u16 {
    let text = text.trim_start();
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let value = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0u32, |n, d| {
            n.wrapping_mul(10).wrapping_add(u32::from(d - b'0'))
        });
    let value = if negative {
        value.wrapping_neg()
    } else {
        value
    };
    (value & 0xFFFF) as u16
}

/// Cuts `text` to the first [`MAX_ADDRESS`] bytes, at a character boundary.
fn capped(text: &str) -> &str {
    let mut end = text.len().min(MAX_ADDRESS);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Strips `prefix` from the start of `text`, ignoring case.
fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

/// A TCPObject's address (`Net::openConnectTo`): `ip:` in front is dropped, the port number follows the first `:` (28000 when there
/// is none). `None` for what TGE opens no socket for (`ipx:` addresses and `broadcast`): the connect does nothing.
pub(crate) fn tcp_target(text: &str) -> Option<Target> {
    target(text, DEFAULT_PORT, false)
}

/// An HTTPObject's address: `https://` makes the connection TLS (port 443 when none is given), `http://` is dropped (port 80
/// when none is given); anything after a `/` that follows either is left out. Without either it is a TCPObject's address.
pub(crate) fn http_target(text: &str) -> Option<Target> {
    let (rest, port, tls) = if let Some(rest) = strip_prefix_ignore_case(text, "https://") {
        (rest, 443, true)
    } else if let Some(rest) = strip_prefix_ignore_case(text, "http://") {
        (rest, 80, false)
    } else {
        return tcp_target(text);
    };
    let rest = rest.split('/').next().unwrap_or_default();
    target(rest, port, tls)
}

fn target(text: &str, default_port: u16, tls: bool) -> Option<Target> {
    let text = capped(text);
    if strip_prefix_ignore_case(text, "ipx:").is_some() {
        return None;
    }
    let text = strip_prefix_ignore_case(text, "ip:").unwrap_or(text);
    let (host, port) = match text.split_once(':') {
        Some((host, port)) => (host, port_number(port)),
        None => (text, default_port),
    };
    if host.eq_ignore_ascii_case("broadcast") {
        return None;
    }
    let host = match host.parse::<Ipv4Addr>() {
        Ok(ip) => Host::Ip(ip),
        Err(_) if usable_name(host) => Host::Name(host.to_string()),
        Err(_) => Host::Unusable,
    };
    Some(Target { host, port, tls })
}

/// An accepted peer's address as TGE's `Net::addressToString` writes it for `onConnectRequest`.
pub(crate) fn address_text(address: std::net::SocketAddr) -> String {
    match address {
        std::net::SocketAddr::V4(v4) => format!("IP:{}:{}", v4.ip(), v4.port()),
        std::net::SocketAddr::V6(v6) => format!("IP:{}:{}", v6.ip(), v6.port()),
    }
}

#[cfg(test)]
#[path = "address_tests.rs"]
mod tests;
