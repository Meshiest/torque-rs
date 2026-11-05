//! The signaling between a browser and the server, carried as text messages on the browser's game socket (binary messages
//! stay datagrams).
//!
//! Server to browser: [`HELLO`] once the socket opens (a server without data channels sends nothing, and a browser that
//! hears nothing stays on the socket), [`ANSWER`] with the SDP answer to an offer, [`BUSY`] when the server takes no more
//! data channels (the browser stays on the socket), and [`CLOSED`] when the server dropped the browser's data channel
//! (the browser closes its peer connection and goes on over the socket). Browser to server: [`OFFER`] with an SDP offer
//! (again later to renegotiate), [`CANDIDATE`] with a trickled ICE candidate (the server is ICE-lite and learns the
//! browser's address from its checks, so these are counted and dropped), and [`CLOSED`] when the browser gave up on its
//! data channel. A kind and its body are split by the first newline.

/// The server takes data channels (server to browser, no body).
pub const HELLO: &str = "rtc";
/// An SDP offer (browser to server).
pub const OFFER: &str = "offer";
/// The SDP answer (server to browser).
pub const ANSWER: &str = "answer";
/// A trickled ICE candidate (browser to server; dropped).
pub const CANDIDATE: &str = "candidate";
/// The server takes no more data channels now (server to browser, no body).
pub const BUSY: &str = "busy";
/// The data channel is closed (either way, no body).
pub const CLOSED: &str = "closed";
/// The data channel's label: the one channel a peer may open.
pub const CHANNEL_LABEL: &str = "game";

/// One signaling message from a browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal<'a> {
    Offer(&'a str),
    Candidate,
    Closed,
}

/// Reads a browser's signaling message (None: not one).
pub fn parse(text: &str) -> Option<Signal<'_>> {
    let (kind, body) = text.split_once('\n').unwrap_or((text, ""));
    match kind {
        OFFER if !body.is_empty() => Some(Signal::Offer(body)),
        CANDIDATE => Some(Signal::Candidate),
        CLOSED if body.is_empty() => Some(Signal::Closed),
        _ => None,
    }
}

/// A message of `kind` carrying `body`.
pub fn message(kind: &str, body: &str) -> String {
    if body.is_empty() {
        return kind.to_string();
    }
    format!("{kind}\n{body}")
}

/// Whether an SDP offer asks for nothing but data channels: every media section is `m=application`. (Voice will add audio
/// sections; until then an offer with any other section is refused.)
pub fn data_only(sdp: &str) -> bool {
    let mut sections = sdp.lines().filter(|l| l.starts_with("m="));
    let mut any = false;
    sections.all(|l| {
        any = true;
        l.starts_with("m=application ")
    }) && any
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signals_read_and_written() {
        assert_eq!(parse("offer\nv=0"), Some(Signal::Offer("v=0")));
        assert_eq!(parse("offer"), None);
        assert_eq!(parse("offer\n"), None);
        assert_eq!(
            parse("candidate\ncandidate:1 1 udp ..."),
            Some(Signal::Candidate)
        );
        assert_eq!(parse("closed"), Some(Signal::Closed));
        assert_eq!(parse("closed\nx"), None);
        assert_eq!(parse("answer\nv=0"), None, "only the server answers");
        assert_eq!(parse(""), None);
        assert_eq!(message(ANSWER, "v=0"), "answer\nv=0");
        assert_eq!(message(HELLO, ""), "rtc");
        assert!(data_only(
            "v=0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n"
        ));
        assert!(!data_only("v=0\r\n"));
        assert!(!data_only(
            "m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n"
        ));
    }
}
