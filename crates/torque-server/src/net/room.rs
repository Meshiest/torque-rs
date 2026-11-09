//! The room protocol (the `rooms` feature): how a player who hosts a game and the players who join it find each other
//! through a room service, with no game traffic through it.
//!
//! A host opens a room and is given its code; a joiner names the code and the service puts it in touch with the host. From
//! then on the service carries only the signaling of one WebRTC data channel between them ([`SIGNALS`]: an SDP offer and
//! answer, trickled ICE candidates, a refusal, a close), and the game's datagrams go straight between the two over that
//! channel. A room has one host and any number of joiners (a star: each joiner's channel goes to the host alone).
//!
//! Every message is UTF-8 text sent as one binary WebSocket message of at most [`MAX_MESSAGE`] bytes: a head line (a verb
//! and its words, split by single spaces) and, for the verbs that carry one, a body after the first newline.
//!
//! To the service:
//! - `host`: open a room for this socket (one a socket).
//! - `join <code>`: join the room of `code`.
//! - `to <peer>` + body: a signaling message from the host to its joiner `peer`.
//! - `signal` + body: a signaling message from a joiner to its host.
//! - `drop <peer>`: the host is done with `peer`'s signaling (its socket is closed).
//! - `ping`: keeps the socket open (answered with `pong`).
//!
//! From the service:
//! - `room <code> <seconds>` + body: the room is open for at most `seconds`; the body is the ICE servers (a JSON array for
//!   `RTCPeerConnection`'s `iceServers`: STUN URLs, and TURN URLs with short-lived credentials).
//! - `joined <peer>` + body: the joiner is in the room as `peer`; the body is the ICE servers.
//! - `peer <peer> <ip>`: to the host, a joiner came from `ip` (the address its socket comes from; the host's per-address
//!   limits and bans key on it).
//! - `from <peer>` + body: to the host, a signaling message from `peer`.
//! - `signal` + body: to a joiner, a signaling message from the host.
//! - `left <peer>`: to the host, `peer`'s socket closed (its channel, if open, carries on).
//! - `closed <why>`: to a joiner, its signaling is over (`host-left`, `dropped`, `timeout`); the socket closes.
//! - `expired`: to the host, the room's time is up; no one else can join (the game carries on).
//! - `refused <why>`: a request was turned away (`no-room`, `full`, `rate`, `busy`, `bad`).
//! - `pong`.

use std::net::Ipv4Addr;

/// The largest message either way, in bytes (an SDP offer is about a kilobyte).
pub const MAX_MESSAGE: usize = 8 << 10;
/// The characters of a room code: 32 letters and digits with none that read alike (no I, L, O, U).
pub const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// The characters of a room code: 5 bits each, 80 bits in all. A code is the room's only secret, so it must not be guessed
/// however many joins a flood spread over the internet can try: at 2^80 codes over the service's most live rooms
/// (1,024, 2^10), a guess hits about once in 2^70 tries, which no join rate can reach, and the service needs no stop that
/// would refuse every joiner at once.
pub const CODE_LEN: usize = 16;

/// The bits of a room code ([`CODE_LEN`] characters of the 32 of [`CODE_ALPHABET`]).
pub const CODE_BITS: u32 = CODE_LEN as u32 * 5;

/// The signaling messages the service carries between a host and a joiner (the data channels' own,
/// `net::web::rtc::signal`): a body must begin with one of them, so nothing else (a game's datagrams) can ride through
/// the service.
pub const SIGNALS: [&str; 5] = ["offer", "answer", "candidate", "busy", "closed"];

/// A message to the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToService<'a> {
    Host,
    /// The code as given (read with [`read_code`]).
    Join(&'a str),
    To(u32, &'a str),
    Signal(&'a str),
    Drop(u32),
    Ping,
}

/// A message from the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FromService<'a> {
    Room {
        code: &'a str,
        seconds: u64,
        ice: &'a str,
    },
    Joined {
        peer: u32,
        ice: &'a str,
    },
    Peer {
        peer: u32,
        ip: Ipv4Addr,
    },
    From {
        peer: u32,
        body: &'a str,
    },
    Signal(&'a str),
    Left(u32),
    Closed(&'a str),
    Expired,
    Refused(&'a str),
    Pong,
}

/// A message's head line split into its verb and words, and its body (empty when it has none).
fn split(text: &str) -> (&str, Vec<&str>, Option<&str>) {
    let (head, body) = match text.split_once('\n') {
        Some((h, b)) => (h, Some(b)),
        None => (text, None),
    };
    let mut words = head.split(' ');
    let verb = words.next().unwrap_or("");
    (verb, words.collect(), body)
}

/// A peer number: decimal digits only, no sign or leading zero, 1 or more.
fn peer_number(text: &str) -> Option<u32> {
    if text.is_empty() || text.len() > 10 || text.starts_with('0') {
        return None;
    }
    if !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// The longest SDP an offer or answer carries.
pub const MAX_SDP: usize = 4 << 10;
/// The longest candidate line.
pub const MAX_CANDIDATE: usize = 512;

/// Whether `body` is a signaling message ([`SIGNALS`]) of its kind's shape, so that nothing else rides through the
/// service: an offer or answer is an SDP (`v=0` first, at most [`MAX_SDP`] bytes, every line `<letter>=` and printable
/// ASCII), a candidate is one `candidate:` line of at most [`MAX_CANDIDATE`] bytes (or none: no more), and `busy` and
/// `closed` have no body.
pub fn is_signal(body: &str) -> bool {
    let (kind, rest) = match body.split_once('\n') {
        Some((kind, rest)) => (kind, Some(rest)),
        None => (body, None),
    };
    match kind {
        "offer" | "answer" => rest.is_some_and(is_sdp),
        "candidate" => rest.is_none_or(|line| line.is_empty() || is_candidate_line(line)),
        "busy" | "closed" => rest.is_none(),
        _ => false,
    }
}

/// Printable ASCII (and tabs).
fn printable(text: &str) -> bool {
    text.bytes()
        .all(|b| b == b'\t' || (0x20..0x7f).contains(&b))
}

fn is_sdp(sdp: &str) -> bool {
    sdp.len() <= MAX_SDP
        && sdp.starts_with("v=0")
        && sdp.split('\n').all(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let b = line.as_bytes();
            line.is_empty()
                || (b.len() >= 2 && b[0].is_ascii_lowercase() && b[1] == b'=' && printable(line))
        })
}

fn is_candidate_line(line: &str) -> bool {
    let line = line.strip_prefix("a=").unwrap_or(line);
    line.len() <= MAX_CANDIDATE && line.starts_with("candidate:") && printable(line)
}

impl<'a> ToService<'a> {
    /// Reads a message to the service (None: not one; a body must be a signaling message).
    pub fn parse(text: &'a str) -> Option<ToService<'a>> {
        let (verb, words, body) = split(text);
        let signal = |b: Option<&'a str>| b.filter(|b| is_signal(b));
        match (verb, words.as_slice(), body) {
            ("host", [], None) => Some(ToService::Host),
            ("ping", [], None) => Some(ToService::Ping),
            ("join", [code], None) => Some(ToService::Join(code)),
            ("drop", [peer], None) => Some(ToService::Drop(peer_number(peer)?)),
            ("to", [peer], body) => Some(ToService::To(peer_number(peer)?, signal(body)?)),
            ("signal", [], body) => Some(ToService::Signal(signal(body)?)),
            _ => None,
        }
    }

    pub fn write(&self) -> String {
        match self {
            ToService::Host => "host".into(),
            ToService::Ping => "ping".into(),
            ToService::Join(code) => format!("join {code}"),
            ToService::Drop(peer) => format!("drop {peer}"),
            ToService::To(peer, body) => format!("to {peer}\n{body}"),
            ToService::Signal(body) => format!("signal\n{body}"),
        }
    }
}

impl<'a> FromService<'a> {
    /// Reads a message from the service (None: not one).
    pub fn parse(text: &'a str) -> Option<FromService<'a>> {
        let (verb, words, body) = split(text);
        match (verb, words.as_slice(), body) {
            ("room", [code, seconds], Some(ice)) => Some(FromService::Room {
                code,
                seconds: seconds.parse().ok()?,
                ice,
            }),
            ("joined", [peer], Some(ice)) => Some(FromService::Joined {
                peer: peer_number(peer)?,
                ice,
            }),
            ("peer", [peer, ip], None) => Some(FromService::Peer {
                peer: peer_number(peer)?,
                ip: ip.parse().ok()?,
            }),
            ("from", [peer], Some(body)) if is_signal(body) => Some(FromService::From {
                peer: peer_number(peer)?,
                body,
            }),
            ("signal", [], Some(body)) if is_signal(body) => Some(FromService::Signal(body)),
            ("left", [peer], None) => Some(FromService::Left(peer_number(peer)?)),
            ("closed", [why], None) => Some(FromService::Closed(why)),
            ("expired", [], None) => Some(FromService::Expired),
            ("refused", [why], None) => Some(FromService::Refused(why)),
            ("pong", [], None) => Some(FromService::Pong),
            _ => None,
        }
    }

    pub fn write(&self) -> String {
        match self {
            FromService::Room { code, seconds, ice } => format!("room {code} {seconds}\n{ice}"),
            FromService::Joined { peer, ice } => format!("joined {peer}\n{ice}"),
            FromService::Peer { peer, ip } => format!("peer {peer} {ip}"),
            FromService::From { peer, body } => format!("from {peer}\n{body}"),
            FromService::Signal(body) => format!("signal\n{body}"),
            FromService::Left(peer) => format!("left {peer}"),
            FromService::Closed(why) => format!("closed {why}"),
            FromService::Expired => "expired".into(),
            FromService::Refused(why) => format!("refused {why}"),
            FromService::Pong => "pong".into(),
        }
    }
}

/// A room code as a player may type it, read: case-blind, with spaces and dashes left out, and the letters that read like
/// digits taken as them (`O` as 0, `I` and `L` as 1). None: not [`CODE_LEN`] characters of [`CODE_ALPHABET`].
pub fn read_code(text: &str) -> Option<String> {
    if text.len() > 4 * CODE_LEN {
        return None;
    }
    let mut code = String::with_capacity(CODE_LEN);
    for c in text.chars() {
        let c = match c.to_ascii_uppercase() {
            ' ' | '-' => continue,
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        if !c.is_ascii() || !CODE_ALPHABET.contains(&(c as u8)) {
            return None;
        }
        code.push(c);
    }
    (code.len() == CODE_LEN).then_some(code)
}

/// A code's form for people: groups of four (`ABCD-EFGH-JKMN-PQRS`).
pub fn show_code(code: &str) -> String {
    if code.len() != CODE_LEN || !code.is_ascii() {
        return code.to_string();
    }
    code.as_bytes()
        .chunks(4)
        .map(|g| String::from_utf8_lossy(g).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// A code made from random bytes (the service draws them from the system's random source): each character from one
/// byte's low five bits, so every character is equally likely.
pub fn code_from(bytes: &[u8; CODE_LEN]) -> String {
    bytes
        .iter()
        .map(|b| char::from(CODE_ALPHABET[usize::from(b & 31)]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_to_the_service_read_and_written() {
        for m in [
            ToService::Host,
            ToService::Ping,
            ToService::Join("ABCD2345"),
            ToService::Drop(7),
            ToService::To(3, "answer\nv=0"),
            ToService::Signal("candidate\ncandidate:1 1 udp 1 1.2.3.4 5 typ host"),
            ToService::Signal("closed"),
        ] {
            assert_eq!(ToService::parse(&m.write()), Some(m));
        }
        for bad in [
            "",
            "host ",
            "host\n",
            "join",
            "join a b",
            "to 0\noffer\nv=0",
            "to 01\noffer\nv=0",
            "to -1\noffer\nv=0",
            "to 99999999999\noffer\nv=0",
            "to 1",
            "to 1\n",
            "to 1\nhello",
            "signal\nGET / HTTP/1.1",
            "signal\nofferx\nv=0",
            "signal",
            "drop x",
            "HOST",
        ] {
            assert_eq!(ToService::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn messages_from_the_service_read_and_written() {
        for m in [
            FromService::Room {
                code: "ABCD2345",
                seconds: 3600,
                ice: "[]",
            },
            FromService::Joined { peer: 1, ice: "[]" },
            FromService::Peer {
                peer: 4,
                ip: Ipv4Addr::new(192, 0, 2, 1),
            },
            FromService::From {
                peer: 4,
                body: "offer\nv=0",
            },
            FromService::Signal("answer\nv=0"),
            FromService::Left(4),
            FromService::Closed("host-left"),
            FromService::Expired,
            FromService::Refused("no-room"),
            FromService::Pong,
        ] {
            assert_eq!(FromService::parse(&m.write()), Some(m));
        }
        assert_eq!(FromService::parse("from 1\nbinary"), None);
        assert_eq!(FromService::parse("peer 1 ::1"), None);
    }

    #[test]
    fn only_signaling_of_its_shape_rides_through() {
        let sdp = "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n";
        for good in [
            format!("offer\n{sdp}"),
            format!("answer\n{sdp}"),
            "candidate\ncandidate:1 1 udp 2122260223 192.0.2.1 5000 typ host generation 0".into(),
            "candidate\na=candidate:1 1 udp 1 192.0.2.1 5000 typ host".into(),
            "candidate".into(),
            "candidate\n".into(),
            "busy".into(),
            "closed".into(),
        ] {
            assert!(is_signal(&good), "{good:?}");
        }
        let long_sdp = format!("v=0\r\na={}\r\n", "x".repeat(MAX_SDP));
        let long_line = format!("candidate:{}", "1".repeat(MAX_CANDIDATE));
        for bad in [
            "offer".to_string(),
            "offer\n".into(),
            "offer\nhello, this is any data at all".into(),
            format!("offer\n{sdp}free text on a line of its own\r\n"),
            format!("offer\n{long_sdp}"),
            format!("answer\n{sdp}a=\u{7f}\r\n"),
            format!("answer\n{sdp}a=caf\u{e9}\r\n"),
            "candidate\nanything at all".into(),
            "candidate\ncandidate:1 1 udp 1 192.0.2.1 5000 typ host\nsecond line".into(),
            format!("candidate\n{long_line}"),
            "busy\npayload".into(),
            "closed\n".into(),
            "game\nbytes".into(),
        ] {
            assert!(!is_signal(&bad), "{bad:?}");
        }
    }

    #[test]
    fn codes_read_as_typed() {
        assert_eq!(
            read_code("abcd-2345-efgh-6789").as_deref(),
            Some("ABCD2345EFGH6789")
        );
        assert_eq!(
            read_code(" ab cd 23 45 ef gh 67 89 ").as_deref(),
            Some("ABCD2345EFGH6789")
        );
        assert_eq!(
            read_code("OIL0-0000-0000-0000").as_deref(),
            Some("0110000000000000")
        );
        assert_eq!(read_code("ABCD2345EFGH678"), None);
        assert_eq!(read_code("ABCD2345EFGH67890"), None);
        assert_eq!(read_code("ABCD-2345"), None, "a code of the old length");
        assert_eq!(read_code("ABCD-2345-EFGH-678U"), None);
        assert_eq!(read_code("ABCD-2345-EFGH-678\u{e9}"), None);
        assert_eq!(read_code(&"-".repeat(100)), None);
        assert_eq!(show_code("ABCD2345EFGH6789"), "ABCD-2345-EFGH-6789");
        let mut bytes = [0u8; CODE_LEN];
        bytes[..8].copy_from_slice(&[0, 31, 32, 255, 10, 1, 2, 3]);
        assert_eq!(code_from(&bytes), "0Z0ZA12300000000");
        assert!(read_code(&code_from(&[7; CODE_LEN])).is_some());
    }

    #[test]
    fn a_code_holds_at_least_64_bits() {
        assert_eq!(CODE_ALPHABET.len(), 32, "five bits a character");
        const { assert!(CODE_BITS >= 64) };
        assert_eq!(CODE_BITS, 80);
        assert_eq!(
            read_code(&code_from(&[31; CODE_LEN])).map(|c| c.len()),
            Some(CODE_LEN)
        );
    }
}
