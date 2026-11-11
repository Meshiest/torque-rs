//! One WebTransport session over HTTP/3 on one QUIC connection, as a state machine over the connection's streams: it is
//! handed the streams' bytes and gives back what to write, stop or close ([`Action`]), and nothing else. Either role.
//!
//! **Server.** It opens its control, QPACK encoder and QPACK decoder streams and sends SETTINGS. It takes the peer's three
//! streams (any other unidirectional stream is stopped; a second control, encoder or decoder stream closes the connection),
//! reads the peer's SETTINGS, and takes one bidirectional stream, whose first frame must be HEADERS with an extended
//! CONNECT for `webtransport` ([`judge`]): the method, the protocol, the scheme, the authority (an IP address, `localhost`
//! or one of the setup's names, as the web transport's `Host` check; a name an attacker pointed at this server does not
//! reach it, so a page cannot pass the origin check with a host of its own), the path against the setup's, and the
//! `origin` against the setup's [`Origins`] when a browser sends one (a request with no origin is not a browser page's).
//! Accepted, it answers `:status 200` and the session is established, its datagrams marked by the request stream's quarter
//! stream id; refused, it answers 400, 403, 404 or 421 and finishes the stream, and nothing is established. The CONNECT is
//! held until the peer's SETTINGS have come, whichever stream the packets carried first.
//!
//! **Client.** The same streams; once the server's SETTINGS allow it (HTTP datagrams and WebTransport), the CONNECT goes
//! on a bidirectional stream, and a `2xx` answer establishes the session.
//!
//! **Caps.** A stream's unread bytes are held up to [`MAX_STREAM_BUFFER`] (past it: `H3_EXCESSIVE_LOAD`); a frame's
//! payload up to [`frames::MAX_FRAME_PAYLOAD`]; a field section as `qpack` caps it; the peer's QPACK encoder stream may carry
//! nothing (no dynamic table) and its decoder stream only a few bytes. A critical stream that ends closes the connection.
//! At most [`MAX_PEER_STREAMS`] of the peer's streams are tracked at once (the connection allows four; a stopped one is
//! forgotten as it is stopped), past which the connection is closed.

use std::collections::{HashMap, VecDeque};

use quinn_proto::StreamId;

use super::frames::{self, Capsule, FrameRead, error, frame, setting, stream};
use super::qpack::{self, Fields, QpackError};

/// The most unread bytes held for one of the peer's streams.
pub const MAX_STREAM_BUFFER: usize = 16 << 10;
/// The most bytes the peer's QPACK decoder stream may carry in all (stream cancellations are two bytes each).
const MAX_DECODER_BYTES: usize = 64;
/// The longest `:authority` taken.
const MAX_AUTHORITY: usize = 255;
/// The most of the peer's streams tracked at once.
pub const MAX_PEER_STREAMS: usize = 8;

/// Which pages' CONNECTs a server takes, by the `origin` a browser sends with it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Origins {
    /// The server's own page: an origin whose host and port are the CONNECT's `:authority` (which must be one of the
    /// server's own hosts), over `https`, or over `http` from `localhost` or a loopback address alone (the one plain origin
    /// browsers treat as secure).
    #[default]
    SameHost,
    /// These origins only (`https://games.example:8443`, compared without case and a trailing `/`).
    List(Vec<String>),
    /// Any page.
    Any,
}

impl Origins {
    /// Whether a page at `origin` may open a session to `authority`.
    #[must_use]
    pub fn allows(&self, origin: &str, authority: &str) -> bool {
        let origin = origin.trim().trim_end_matches('/');
        match self {
            Origins::Any => true,
            Origins::List(list) => list
                .iter()
                .any(|o| o.trim().trim_end_matches('/').eq_ignore_ascii_case(origin)),
            Origins::SameHost => {
                let Some((scheme, rest)) = origin.split_once("://") else {
                    return false;
                };
                let same = rest.eq_ignore_ascii_case(authority);
                let plain_ok =
                    scheme.eq_ignore_ascii_case("http") && is_loopback_host(host_of(rest));
                same && (scheme.eq_ignore_ascii_case("https") || plain_ok)
            }
        }
    }
}

/// Whether `host` is this computer's own name for a browser: `localhost` or a loopback address.
fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Whether a CONNECT may name `host` as its `:authority`'s host: an IP address, `localhost`, or one of `hosts` (the
/// server's own names). A name an attacker's DNS points at this server names nobody here.
fn known_host(host: &str, hosts: &[String]) -> bool {
    host.parse::<std::net::IpAddr>().is_ok()
        || host.eq_ignore_ascii_case("localhost")
        || hosts.iter().any(|h| h.eq_ignore_ascii_case(host))
}

/// The host of `name:port`, `[v6]:port` or `name` (no path).
fn host_of(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split_once(']').map_or(rest, |(inside, _)| inside);
    }
    authority
        .rsplit_once(':')
        .filter(|(_, port)| port.bytes().all(|b| b.is_ascii_digit()))
        .map_or(authority, |(host, _)| host)
}

/// What a server's sessions require of a CONNECT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setup {
    pub origins: Origins,
    /// The `:path` a CONNECT must name (`/`).
    pub path: String,
    /// The names a CONNECT's `:authority` may carry besides IP addresses and `localhost` (`games.example`): the server's
    /// own, as the web transport's `Access::hosts`.
    pub hosts: Vec<String>,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup {
            origins: Origins::SameHost,
            path: "/".to_string(),
            hosts: Vec::new(),
        }
    }
}

/// What a client's CONNECT names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dial {
    /// The `:authority` (`host:port`).
    pub authority: String,
    /// The `:path` (`/`).
    pub path: String,
    /// An `origin` to send, as a browser's page would; none from a native client.
    pub origin: Option<String>,
}

/// What the driver does for the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Write these bytes on this stream (ours, or the peer's bidirectional one).
    Write { stream: StreamId, data: Vec<u8> },
    /// Finish this stream after what was written.
    Finish(StreamId),
    /// Stop reading this stream of the peer's with this error code.
    Stop { stream: StreamId, code: u64 },
    /// Close the connection with this HTTP/3 error code.
    Close { code: u64, reason: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Client,
    Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A unidirectional stream whose type has not come yet.
    Typing,
    Control,
    Encoder,
    Decoder,
    /// Stopped: its bytes are dropped.
    Ignored,
    /// The CONNECT stream: the peer's on a server, ours on a client.
    Request,
}

#[derive(Debug)]
struct Incoming {
    kind: Kind,
    buf: Vec<u8>,
    /// The control stream's SETTINGS, or the request stream's HEADERS, has been read.
    head_done: bool,
}

/// Why the session ended, in words for the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum End {
    /// The peer closed the session (a close capsule, or its CONNECT stream ended).
    Closed { code: u32, reason: String },
    /// The server refused the CONNECT with this status.
    Refused(u16),
    /// A protocol error: the connection is closed with the code of the `Close` action.
    Protocol(&'static str),
}

/// The session state machine (module docs).
#[derive(Debug)]
pub struct Session {
    role: Role,
    setup: Option<Setup>,
    dial: Option<Dial>,
    /// Our control stream, once opened.
    control: Option<StreamId>,
    /// Our request stream (the client's), once opened.
    request: Option<StreamId>,
    /// The peer's request stream (the server's view).
    peer_request: Option<StreamId>,
    peer_control: Option<StreamId>,
    peer_encoder: Option<StreamId>,
    peer_decoder: Option<StreamId>,
    peer_settings: Option<frames::Settings>,
    /// A CONNECT read before the peer's SETTINGS.
    pending_connect: Option<Fields>,
    connect_sent: bool,
    streams: HashMap<StreamId, Incoming>,
    actions: VecDeque<Action>,
    decoder_bytes: usize,
    established: Option<u64>,
    end: Option<End>,
    /// The request carried `sec-webtransport-http3-draft02` (the answer echoes the draft).
    draft02: bool,
}

/// The settings both ends send: extended CONNECT, HTTP datagrams (both numberings) and one WebTransport session (both
/// drafts' settings).
fn our_settings() -> Vec<(u64, u64)> {
    vec![
        (setting::ENABLE_CONNECT_PROTOCOL, 1),
        (setting::H3_DATAGRAM, 1),
        (setting::H3_DATAGRAM_DRAFT, 1),
        (setting::ENABLE_WEBTRANSPORT, 1),
        (setting::WT_MAX_SESSIONS, 1),
    ]
}

fn has_datagrams(settings: &[(u64, u64)]) -> bool {
    frames::setting_value(settings, setting::H3_DATAGRAM) == Some(1)
        || frames::setting_value(settings, setting::H3_DATAGRAM_DRAFT) == Some(1)
}

fn has_webtransport(settings: &[(u64, u64)]) -> bool {
    frames::setting_value(settings, setting::ENABLE_WEBTRANSPORT) == Some(1)
        || frames::setting_value(settings, setting::WT_MAX_SESSIONS).is_some_and(|n| n >= 1)
}

/// The value of the one field `name` in `fields`; Err when it appears twice.
fn one<'a>(fields: &'a Fields, name: &str) -> Result<Option<&'a str>, ()> {
    let mut found = None;
    for (n, v) in fields {
        if n == name {
            if found.is_some() {
                return Err(());
            }
            found = Some(v.as_str());
        }
    }
    Ok(found)
}

/// Judges a CONNECT's fields under `setup`: Ok with whether it carried the draft-02 marker, or the status to refuse it with
/// (400 malformed or not a WebTransport CONNECT, 421 an authority that is not one of this server's hosts, 403 a page from
/// elsewhere, 404 another path).
///
/// # Errors
/// The status to answer.
pub fn judge(fields: &Fields, setup: &Setup) -> Result<bool, u16> {
    let field = |name: &str| one(fields, name).map_err(|()| 400u16);
    if field(":method")? != Some("CONNECT") || field(":protocol")? != Some("webtransport") {
        return Err(400);
    }
    if field(":scheme")? != Some("https") {
        return Err(400);
    }
    let authority = field(":authority")?.ok_or(400u16)?;
    if authority.is_empty() || authority.len() > MAX_AUTHORITY || authority.contains('/') {
        return Err(400);
    }
    if !known_host(host_of(authority), &setup.hosts) {
        return Err(421);
    }
    if field(":path")? != Some(setup.path.as_str()) {
        return Err(404);
    }
    if let Some(origin) = field("origin")?
        && !setup.origins.allows(origin, authority)
    {
        return Err(403);
    }
    Ok(field("sec-webtransport-http3-draft02")?.is_some())
}

impl Session {
    /// A server's session under `setup`.
    #[must_use]
    pub fn server(setup: Setup) -> Session {
        Session::new(Role::Server, Some(setup), None)
    }

    /// A client's session for `dial`.
    #[must_use]
    pub fn client(dial: Dial) -> Session {
        Session::new(Role::Client, None, Some(dial))
    }

    fn new(role: Role, setup: Option<Setup>, dial: Option<Dial>) -> Session {
        Session {
            role,
            setup,
            dial,
            control: None,
            request: None,
            peer_request: None,
            peer_control: None,
            peer_encoder: None,
            peer_decoder: None,
            peer_settings: None,
            pending_connect: None,
            connect_sent: false,
            streams: HashMap::new(),
            actions: VecDeque::new(),
            decoder_bytes: 0,
            established: None,
            end: None,
            draft02: false,
        }
    }

    /// Our three unidirectional streams, opened by the driver: the stream types and our SETTINGS go out.
    pub fn local_streams(&mut self, control: StreamId, encoder: StreamId, decoder: StreamId) {
        self.control = Some(control);
        let mut data = Vec::with_capacity(64);
        frames::put_varint(&mut data, stream::CONTROL);
        frames::put_frame(
            &mut data,
            frame::SETTINGS,
            &frames::put_settings(&our_settings()),
        );
        self.write(control, data);
        let mut data = Vec::new();
        frames::put_varint(&mut data, stream::QPACK_ENCODER);
        self.write(encoder, data);
        let mut data = Vec::new();
        frames::put_varint(&mut data, stream::QPACK_DECODER);
        self.write(decoder, data);
    }

    /// The client's request stream, opened by the driver; the CONNECT goes once the server's SETTINGS allow it.
    pub fn request_stream(&mut self, id: StreamId) {
        self.request = Some(id);
        self.streams.insert(
            id,
            Incoming {
                kind: Kind::Request,
                buf: Vec::new(),
                head_done: false,
            },
        );
        self.maybe_connect();
    }

    /// Whether the client's CONNECT still waits for the server's SETTINGS.
    #[must_use]
    pub fn waiting_for_settings(&self) -> bool {
        self.role == Role::Client && !self.connect_sent
    }

    /// A stream the peer opened.
    pub fn peer_stream(&mut self, id: StreamId, bidi: bool) {
        if self.end.is_some() {
            return;
        }
        if self.streams.len() >= MAX_PEER_STREAMS {
            return self.fail(error::H3_STREAM_CREATION_ERROR, "too many streams");
        }
        let kind = if bidi {
            match self.role {
                Role::Server if self.peer_request.is_none() => {
                    self.peer_request = Some(id);
                    Kind::Request
                }
                // A server opens no bidirectional stream; a second request has no place in one session.
                Role::Server | Role::Client => {
                    return self.fail(error::H3_STREAM_CREATION_ERROR, "a second request stream");
                }
            }
        } else {
            Kind::Typing
        };
        self.streams.insert(
            id,
            Incoming {
                kind,
                buf: Vec::new(),
                head_done: false,
            },
        );
    }

    /// Bytes from the peer on `id` (`fin`: the stream ends after them).
    pub fn data(&mut self, id: StreamId, bytes: &[u8], fin: bool) {
        if self.end.is_some() {
            return;
        }
        let Some(incoming) = self.streams.get_mut(&id) else {
            return;
        };
        if incoming.kind == Kind::Ignored {
            return;
        }
        if incoming.buf.len() + bytes.len() > MAX_STREAM_BUFFER {
            return self.fail(
                error::H3_EXCESSIVE_LOAD,
                "a stream's unread bytes passed the cap",
            );
        }
        incoming.buf.extend_from_slice(bytes);
        self.process(id);
        if fin && self.end.is_none() {
            self.finished(id);
        }
    }

    /// The peer reset stream `id`.
    pub fn reset(&mut self, id: StreamId) {
        if self.end.is_some() {
            return;
        }
        self.finished(id);
    }

    fn finished(&mut self, id: StreamId) {
        let Some(incoming) = self.streams.get(&id) else {
            return;
        };
        match incoming.kind {
            Kind::Control | Kind::Encoder | Kind::Decoder => {
                self.fail(error::H3_CLOSED_CRITICAL_STREAM, "a critical stream ended");
            }
            Kind::Request => {
                self.streams.remove(&id);
                if self.end.is_none() {
                    self.end = Some(End::Closed {
                        code: 0,
                        reason: "the session's stream ended".to_string(),
                    });
                }
            }
            Kind::Typing | Kind::Ignored => {
                self.streams.remove(&id);
            }
        }
    }

    fn process(&mut self, id: StreamId) {
        loop {
            let Some(incoming) = self.streams.get_mut(&id) else {
                return;
            };
            match incoming.kind {
                Kind::Typing => {
                    let Some((kind, rest)) = frames::varint(&incoming.buf) else {
                        // A type is at most eight bytes; more without one is not a stream type.
                        if incoming.buf.len() >= 8 {
                            return self.fail(
                                error::H3_STREAM_CREATION_ERROR,
                                "a stream type that is not one",
                            );
                        }
                        return;
                    };
                    incoming.buf = rest.to_vec();
                    let slot = match kind {
                        stream::CONTROL => (Kind::Control, &mut self.peer_control),
                        stream::QPACK_ENCODER => (Kind::Encoder, &mut self.peer_encoder),
                        stream::QPACK_DECODER => (Kind::Decoder, &mut self.peer_decoder),
                        stream::PUSH => {
                            return self.fail(error::H3_STREAM_CREATION_ERROR, "a push stream");
                        }
                        _ => {
                            // Stopped and forgotten: whatever else comes on it finds no entry.
                            self.streams.remove(&id);
                            self.actions.push_back(Action::Stop {
                                stream: id,
                                code: error::H3_STREAM_CREATION_ERROR,
                            });
                            return;
                        }
                    };
                    if slot.1.is_some() {
                        return self
                            .fail(error::H3_STREAM_CREATION_ERROR, "a second critical stream");
                    }
                    *slot.1 = Some(id);
                    incoming.kind = slot.0;
                }
                Kind::Control => return self.control_frames(id),
                Kind::Encoder => {
                    if !incoming.buf.is_empty() {
                        return self.fail(
                            error::QPACK_ENCODER_STREAM_ERROR,
                            "an encoder instruction with no dynamic table",
                        );
                    }
                    return;
                }
                Kind::Decoder => {
                    self.decoder_bytes += incoming.buf.len();
                    incoming.buf.clear();
                    if self.decoder_bytes > MAX_DECODER_BYTES {
                        return self.fail(
                            error::QPACK_DECODER_STREAM_ERROR,
                            "too many decoder instructions",
                        );
                    }
                    return;
                }
                Kind::Request => return self.request_frames(id),
                Kind::Ignored => return,
            }
        }
    }

    /// Takes one frame off stream `id`'s buffer: its type and payload; None when more bytes are needed or the frame is too
    /// long (which closes the connection).
    fn take_frame(&mut self, id: StreamId) -> Option<(u64, Vec<u8>)> {
        let incoming = self.streams.get_mut(&id)?;
        match frames::read_frame(&incoming.buf) {
            FrameRead::Frame {
                kind,
                payload,
                rest,
            } => {
                let payload = payload.to_vec();
                incoming.buf = rest.to_vec();
                Some((kind, payload))
            }
            FrameRead::Partial => None,
            FrameRead::TooLong => {
                self.fail(error::H3_EXCESSIVE_LOAD, "a frame longer than the cap");
                None
            }
        }
    }

    fn control_frames(&mut self, id: StreamId) {
        while let Some((kind, payload)) = self.take_frame(id) {
            let head_done = self.streams.get(&id).is_some_and(|s| s.head_done);
            match kind {
                frame::SETTINGS if head_done => {
                    return self.fail(error::H3_FRAME_UNEXPECTED, "a second SETTINGS");
                }
                frame::SETTINGS => {
                    let settings = match frames::read_settings(&payload) {
                        Ok(s) => s,
                        Err(_) => {
                            return self
                                .fail(error::H3_SETTINGS_ERROR, "SETTINGS that do not parse");
                        }
                    };
                    if !has_datagrams(&settings) {
                        return self
                            .fail(error::H3_SETTINGS_ERROR, "the peer takes no HTTP datagrams");
                    }
                    if self.role == Role::Client && !has_webtransport(&settings) {
                        return self.fail(
                            error::H3_SETTINGS_ERROR,
                            "the server takes no WebTransport sessions",
                        );
                    }
                    if let Some(s) = self.streams.get_mut(&id) {
                        s.head_done = true;
                    }
                    self.peer_settings = Some(settings);
                    if let Some(fields) = self.pending_connect.take() {
                        self.answer_connect(&fields);
                    }
                    self.maybe_connect();
                }
                _ if !head_done => {
                    return self.fail(
                        error::H3_MISSING_SETTINGS,
                        "a control stream not starting with SETTINGS",
                    );
                }
                frame::DATA | frame::HEADERS => {
                    return self.fail(
                        error::H3_FRAME_UNEXPECTED,
                        "a request frame on the control stream",
                    );
                }
                // GOAWAY, priority updates and unknown frames: nothing to do with one session.
                _ => {}
            }
            if self.end.is_some() {
                return;
            }
        }
    }

    fn request_frames(&mut self, id: StreamId) {
        while let Some((kind, payload)) = self.take_frame(id) {
            let head_done = self.streams.get(&id).is_some_and(|s| s.head_done);
            match kind {
                frame::HEADERS if !head_done => {
                    let fields = match qpack::decode(&payload) {
                        Ok(f) => f,
                        Err(
                            QpackError::SectionTooLarge
                            | QpackError::TooManyFields
                            | QpackError::StringTooLong,
                        ) => {
                            return self
                                .fail(error::H3_EXCESSIVE_LOAD, "a field section past its caps");
                        }
                        Err(_) => {
                            return self.fail(
                                error::QPACK_DECOMPRESSION_FAILED,
                                "a field section that does not decode",
                            );
                        }
                    };
                    if let Some(s) = self.streams.get_mut(&id) {
                        s.head_done = true;
                    }
                    match self.role {
                        Role::Server if self.peer_settings.is_some() => {
                            self.answer_connect(&fields)
                        }
                        Role::Server => self.pending_connect = Some(fields),
                        Role::Client => self.take_response(&fields),
                    }
                }
                frame::DATA if head_done => {
                    for capsule in frames::read_capsules(&payload) {
                        if let Capsule::Close { code, reason } = capsule {
                            self.end = Some(End::Closed { code, reason });
                            return;
                        }
                    }
                }
                frame::DATA => return self.fail(error::H3_FRAME_UNEXPECTED, "DATA before HEADERS"),
                frame::SETTINGS | frame::GOAWAY | frame::WEBTRANSPORT_STREAM => {
                    return self.fail(
                        error::H3_FRAME_UNEXPECTED,
                        "a control frame on the request stream",
                    );
                }
                // Trailers and unknown frames: ignored.
                _ => {}
            }
            if self.end.is_some() {
                return;
            }
        }
    }

    /// The server judges the CONNECT and answers it.
    fn answer_connect(&mut self, fields: &Fields) {
        let Some(stream) = self.peer_request else {
            return;
        };
        let setup = self.setup.clone().unwrap_or_default();
        match judge(fields, &setup) {
            Ok(draft02) => {
                self.draft02 = draft02;
                let mut headers: Vec<(&str, &str)> = vec![(":status", "200")];
                if draft02 {
                    headers.push(("sec-webtransport-http3-draft", "draft02"));
                }
                let mut data = Vec::new();
                frames::put_frame(&mut data, frame::HEADERS, &qpack::encode(&headers));
                self.write(stream, data);
                self.established = Some(stream.index());
            }
            Err(status) => {
                let status = status.to_string();
                let mut data = Vec::new();
                frames::put_frame(
                    &mut data,
                    frame::HEADERS,
                    &qpack::encode(&[(":status", &status)]),
                );
                self.write(stream, data);
                self.actions.push_back(Action::Finish(stream));
                self.end = Some(End::Refused(status.parse().unwrap_or(400)));
            }
        }
    }

    /// The client reads the server's answer.
    fn take_response(&mut self, fields: &Fields) {
        let status = one(fields, ":status")
            .ok()
            .flatten()
            .and_then(|s| s.parse::<u16>().ok());
        match status {
            Some(status) if (200..300).contains(&status) => {
                self.established = self.request.map(|id| id.index());
            }
            Some(status) => self.end = Some(End::Refused(status)),
            None => self.fail(error::H3_MESSAGE_ERROR, "an answer with no status"),
        }
    }

    /// The client sends its CONNECT once the server's SETTINGS are in and its request stream is open.
    fn maybe_connect(&mut self) {
        if self.role != Role::Client || self.connect_sent || self.peer_settings.is_none() {
            return;
        }
        let (Some(stream), Some(dial)) = (self.request, self.dial.as_ref()) else {
            return;
        };
        let mut headers: Vec<(&str, &str)> = vec![
            (":method", "CONNECT"),
            (":protocol", "webtransport"),
            (":scheme", "https"),
            (":authority", &dial.authority),
            (":path", &dial.path),
        ];
        if let Some(origin) = &dial.origin {
            headers.push(("origin", origin));
        }
        let mut data = Vec::new();
        frames::put_frame(&mut data, frame::HEADERS, &qpack::encode(&headers));
        self.connect_sent = true;
        self.write(stream, data);
    }

    fn write(&mut self, stream: StreamId, data: Vec<u8>) {
        self.actions.push_back(Action::Write { stream, data });
    }

    fn fail(&mut self, code: u64, reason: &'static str) {
        if self.end.is_some() {
            return;
        }
        self.end = Some(End::Protocol(reason));
        self.actions.clear();
        self.actions.push_back(Action::Close { code, reason });
    }

    /// The next thing for the driver to do.
    pub fn poll(&mut self) -> Option<Action> {
        self.actions.pop_front()
    }

    /// The quarter stream id of the session's datagrams (RFC 9297 section 2.1), once the session is established.
    #[must_use]
    pub fn established(&self) -> Option<u64> {
        self.established.filter(|_| self.end.is_none())
    }

    /// Why the session ended, if it did.
    #[must_use]
    pub fn end(&self) -> Option<&End> {
        self.end.as_ref()
    }

    /// The bytes a datagram of this session starts with: its quarter stream id.
    #[must_use]
    pub fn datagram_prefix(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8);
        frames::put_varint(&mut out, self.established.unwrap_or(0));
        out
    }

    /// The payload of a datagram of this session, if `datagram` is one (its quarter stream id is the session's).
    #[must_use]
    pub fn unwrap_datagram<'a>(&self, datagram: &'a [u8]) -> Option<&'a [u8]> {
        let want = self.established()?;
        let (quarter, payload) = frames::varint(datagram)?;
        (quarter == want).then_some(payload)
    }

    /// What to write on the request stream to close the session, with `code` and `reason`, before the connection closes.
    #[must_use]
    pub fn close_capsule(&self, code: u32, reason: &str) -> Option<Action> {
        let stream = self.established.and(self.request.or(self.peer_request))?;
        Some(Action::Write {
            stream,
            data: frames::put_close_capsule(code, reason),
        })
    }

    /// The unread bytes held for the peer's streams, in all (a test's view of the caps).
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.streams.values().map(|s| s.buf.len()).sum()
    }

    /// The peer's streams tracked now.
    #[must_use]
    pub fn tracked(&self) -> usize {
        self.streams.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quinn_proto::{Dir, Side};

    fn uni(side: Side, index: u64) -> StreamId {
        StreamId::new(side, Dir::Uni, index)
    }

    fn bi(side: Side, index: u64) -> StreamId {
        StreamId::new(side, Dir::Bi, index)
    }

    fn setup() -> Setup {
        Setup {
            origins: Origins::SameHost,
            path: "/".into(),
            hosts: vec!["games.example".into()],
        }
    }

    fn dial(origin: Option<&str>) -> Dial {
        Dial {
            authority: "10.0.0.1:28000".into(),
            path: "/".into(),
            origin: origin.map(str::to_string),
        }
    }

    /// Two sessions wired to each other: every Write of one becomes data of the other; the actions that are not writes.
    fn pump(server: &mut Session, client: &mut Session) -> Vec<Action> {
        let mut others = Vec::new();
        for _ in 0..8 {
            let mut moved = false;
            while let Some(a) = server.poll() {
                match a {
                    Action::Write { stream, data } => {
                        client.data(stream, &data, false);
                        moved = true;
                    }
                    other => others.push(other),
                }
            }
            while let Some(a) = client.poll() {
                match a {
                    Action::Write { stream, data } => {
                        server.data(stream, &data, false);
                        moved = true;
                    }
                    other => others.push(other),
                }
            }
            if !moved {
                break;
            }
        }
        others
    }

    fn pair(origin: Option<&str>) -> (Session, Session) {
        let mut server = Session::server(setup());
        let mut client = Session::client(dial(origin));
        server.local_streams(
            uni(Side::Server, 0),
            uni(Side::Server, 1),
            uni(Side::Server, 2),
        );
        client.local_streams(
            uni(Side::Client, 0),
            uni(Side::Client, 1),
            uni(Side::Client, 2),
        );
        for i in 0..3 {
            server.peer_stream(uni(Side::Client, i), false);
            client.peer_stream(uni(Side::Server, i), false);
        }
        client.request_stream(bi(Side::Client, 0));
        server.peer_stream(bi(Side::Client, 0), true);
        (server, client)
    }

    #[test]
    fn a_client_and_a_server_establish_a_session_and_agree_on_the_datagram_prefix() {
        let (mut server, mut client) = pair(Some("https://10.0.0.1:28000"));
        assert!(client.waiting_for_settings());
        let others = pump(&mut server, &mut client);
        assert!(others.is_empty(), "{others:?}");
        assert_eq!(server.established(), Some(0));
        assert_eq!(client.established(), Some(0));
        assert_eq!(server.datagram_prefix(), vec![0]);
        assert_eq!(client.unwrap_datagram(&[0, 7, 8]), Some(&[7, 8][..]));
        assert_eq!(
            client.unwrap_datagram(&[1, 7, 8]),
            None,
            "another session's quarter stream id"
        );
        assert_eq!(client.unwrap_datagram(&[]), None);
        // A close capsule from the client ends the server's session.
        let Some(Action::Write { stream, data }) = client.close_capsule(3, "done") else {
            panic!("a capsule")
        };
        server.data(stream, &data, false);
        assert_eq!(
            server.end(),
            Some(&End::Closed {
                code: 3,
                reason: "done".into()
            })
        );
        assert_eq!(server.established(), None);
    }

    #[test]
    fn a_native_client_with_no_origin_is_taken_and_a_page_from_elsewhere_is_refused() {
        let (mut server, mut client) = pair(None);
        pump(&mut server, &mut client);
        assert_eq!(server.established(), Some(0));
        let (mut server, mut client) = pair(Some("https://evil.example"));
        let others = pump(&mut server, &mut client);
        assert_eq!(others, vec![Action::Finish(bi(Side::Client, 0))]);
        assert_eq!(server.end(), Some(&End::Refused(403)));
        assert_eq!(client.end(), Some(&End::Refused(403)));
        assert_eq!(server.established(), None);
        assert_eq!(client.established(), None);
    }

    #[test]
    fn the_connect_is_held_until_the_settings_come_whichever_order_the_streams_arrive_in() {
        let mut server = Session::server(setup());
        server.local_streams(
            uni(Side::Server, 0),
            uni(Side::Server, 1),
            uni(Side::Server, 2),
        );
        while server.poll().is_some() {}
        let request = bi(Side::Client, 0);
        server.peer_stream(request, true);
        let mut headers = Vec::new();
        frames::put_frame(
            &mut headers,
            frame::HEADERS,
            &qpack::encode(&[
                (":method", "CONNECT"),
                (":protocol", "webtransport"),
                (":scheme", "https"),
                (":authority", "localhost:4433"),
                (":path", "/"),
                ("origin", "http://localhost:4433"),
                ("sec-webtransport-http3-draft02", "1"),
            ]),
        );
        server.data(request, &headers, false);
        assert_eq!(server.established(), None, "no SETTINGS yet");
        assert!(server.poll().is_none());
        let control = uni(Side::Client, 0);
        server.peer_stream(control, false);
        let mut data = vec![0x00];
        frames::put_frame(
            &mut data,
            frame::SETTINGS,
            &frames::put_settings(&our_settings()),
        );
        // One byte at a time: the varints and frames reassemble.
        for b in data {
            server.data(control, &[b], false);
        }
        assert_eq!(server.established(), Some(0));
        let Some(Action::Write { stream, data }) = server.poll() else {
            panic!("the answer")
        };
        assert_eq!(stream, request);
        let FrameRead::Frame { kind, payload, .. } = frames::read_frame(&data) else {
            panic!("a frame")
        };
        assert_eq!(kind, frame::HEADERS);
        let fields = qpack::decode(payload).expect("decodes");
        assert_eq!(fields[0], (":status".to_string(), "200".to_string()));
        assert_eq!(
            fields[1],
            (
                "sec-webtransport-http3-draft".to_string(),
                "draft02".to_string()
            )
        );
    }

    #[test]
    fn judge_checks_every_field() {
        let good: Fields = [
            (":method", "CONNECT"),
            (":protocol", "webtransport"),
            (":scheme", "https"),
            (":authority", "games.example:4433"),
            (":path", "/"),
            ("origin", "https://games.example:4433"),
        ]
        .iter()
        .map(|&(n, v)| (n.to_string(), v.to_string()))
        .collect();
        assert_eq!(judge(&good, &setup()), Ok(false));
        let swap = |name: &str, value: &str| -> Fields {
            good.iter()
                .map(|(n, v)| {
                    (
                        n.clone(),
                        if n == name {
                            value.to_string()
                        } else {
                            v.clone()
                        },
                    )
                })
                .collect()
        };
        assert_eq!(judge(&swap(":method", "GET"), &setup()), Err(400));
        assert_eq!(judge(&swap(":protocol", "websocket"), &setup()), Err(400));
        assert_eq!(judge(&swap(":scheme", "http"), &setup()), Err(400));
        assert_eq!(judge(&swap(":authority", ""), &setup()), Err(400));
        assert_eq!(
            judge(&swap(":authority", &"a".repeat(300)), &setup()),
            Err(400)
        );
        assert_eq!(judge(&swap(":path", "/other"), &setup()), Err(404));
        assert_eq!(
            judge(&swap("origin", "https://other.example"), &setup()),
            Err(403)
        );
        assert_eq!(
            judge(&swap("origin", "https://games.example:8443/"), &setup()),
            Err(403),
            "another port"
        );
        assert_eq!(
            judge(&swap("origin", "https://games.example:4433/"), &setup()),
            Ok(false),
            "a trailing slash"
        );
        assert_eq!(
            judge(&swap("origin", "http://games.example:4433"), &setup()),
            Err(403),
            "plain http"
        );
        assert_eq!(judge(&swap("origin", "null"), &setup()), Err(403));
        assert_eq!(
            judge(&swap("origin", "ftp://games.example"), &setup()),
            Err(403)
        );
        // A host of the attacker's, pointed at this server by their DNS, is not one of ours: 421, origin or not.
        let rebound: Fields = good
            .iter()
            .map(|(n, v)| {
                let v = match n.as_str() {
                    ":authority" => "evil.example:4433",
                    "origin" => "https://evil.example:4433",
                    _ => v,
                };
                (n.clone(), v.to_string())
            })
            .collect();
        assert_eq!(judge(&rebound, &setup()), Err(421));
        let mut rebound_no_origin = rebound.clone();
        rebound_no_origin.retain(|(n, _)| n != "origin");
        assert_eq!(judge(&rebound_no_origin, &setup()), Err(421));
        // IP addresses and localhost are always ours; a plain-http origin passes from them alone.
        assert_eq!(
            judge(&swap(":authority", "203.0.113.5:4433"), &setup()),
            Err(403),
            "the origin names the name"
        );
        let ip: Fields = good
            .iter()
            .map(|(n, v)| {
                let v = match n.as_str() {
                    ":authority" => "203.0.113.5:4433",
                    "origin" => "https://203.0.113.5:4433",
                    _ => v,
                };
                (n.clone(), v.to_string())
            })
            .collect();
        assert_eq!(judge(&ip, &setup()), Ok(false));
        assert!(Origins::SameHost.allows("http://localhost:4433", "localhost:4433"));
        assert!(Origins::SameHost.allows("http://127.0.0.1:4433", "127.0.0.1:4433"));
        assert!(
            !Origins::SameHost.allows("http://10.0.0.5:4433", "10.0.0.5:4433"),
            "plain http off the loopback"
        );
        assert!(
            !Origins::SameHost.allows("https://localhost:4433", "localhost:28000"),
            "another port number"
        );
        let mut twice = good.clone();
        twice.push((":path".into(), "/".into()));
        assert_eq!(judge(&twice, &setup()), Err(400));
        let mut no_origin = good.clone();
        no_origin.retain(|(n, _)| n != "origin");
        assert_eq!(judge(&no_origin, &setup()), Ok(false));
        let list = Setup {
            origins: Origins::List(vec!["https://Site.Example/".into()]),
            path: "/".into(),
            hosts: vec!["games.example".into()],
        };
        assert_eq!(
            judge(&swap("origin", "https://site.example"), &list),
            Ok(false)
        );
        assert_eq!(judge(&good, &list), Err(403));
        let any = Setup {
            origins: Origins::Any,
            path: "/game".into(),
            hosts: vec!["games.example".into()],
        };
        assert_eq!(judge(&swap("origin", "null"), &any), Err(404));
        assert_eq!(judge(&swap(":path", "/game"), &any), Ok(false));
        assert!(Origins::SameHost.allows("https://[::1]:4433", "[::1]:4433"));
        assert!(!Origins::SameHost.allows("https://[::1]:4433", "[::2]:4433"));
        assert!(known_host("::1", &[]));
    }

    #[test]
    fn the_peers_misbehavior_closes_the_connection_with_the_right_code() {
        let close = |f: &dyn Fn(&mut Session)| -> Option<Action> {
            let mut server = Session::server(setup());
            server.local_streams(
                uni(Side::Server, 0),
                uni(Side::Server, 1),
                uni(Side::Server, 2),
            );
            while server.poll().is_some() {}
            f(&mut server);
            let mut last = None;
            while let Some(a) = server.poll() {
                last = Some(a);
            }
            assert!(server.end().is_some() || last.is_none());
            last
        };
        let code = |a: Option<Action>| match a {
            Some(Action::Close { code, .. }) => code,
            other => panic!("not a close: {other:?}"),
        };
        // Two control streams.
        assert_eq!(
            code(close(&|s| {
                s.peer_stream(uni(Side::Client, 0), false);
                s.peer_stream(uni(Side::Client, 1), false);
                s.data(uni(Side::Client, 0), &[0x00], false);
                s.data(uni(Side::Client, 1), &[0x00], false);
            })),
            error::H3_STREAM_CREATION_ERROR
        );
        // A frame that is not SETTINGS first.
        assert_eq!(
            code(close(&|s| {
                s.peer_stream(uni(Side::Client, 0), false);
                s.data(uni(Side::Client, 0), &[0x00, 0x07, 0x01, 0x00], false);
            })),
            error::H3_MISSING_SETTINGS
        );
        // SETTINGS that do not parse, and SETTINGS without datagrams.
        assert_eq!(
            code(close(&|s| {
                s.peer_stream(uni(Side::Client, 0), false);
                s.data(uni(Side::Client, 0), &[0x00, 0x04, 0x01, 0x33], false);
            })),
            error::H3_SETTINGS_ERROR
        );
        assert_eq!(
            code(close(&|s| {
                s.peer_stream(uni(Side::Client, 0), false);
                let mut d = vec![0x00];
                frames::put_frame(
                    &mut d,
                    frame::SETTINGS,
                    &frames::put_settings(&[(setting::ENABLE_CONNECT_PROTOCOL, 1)]),
                );
                s.data(uni(Side::Client, 0), &d, false);
            })),
            error::H3_SETTINGS_ERROR
        );
        // The control stream ends.
        assert_eq!(
            code(close(&|s| {
                s.peer_stream(uni(Side::Client, 0), false);
                s.data(uni(Side::Client, 0), &[0x00], true);
            })),
            error::H3_CLOSED_CRITICAL_STREAM
        );
        // An encoder instruction.
        assert_eq!(
            code(close(&|s| {
                s.peer_stream(uni(Side::Client, 0), false);
                s.data(uni(Side::Client, 0), &[0x02, 0x3f, 0xe1, 0x1f], false);
            })),
            error::QPACK_ENCODER_STREAM_ERROR
        );
        // Too much on the decoder stream.
        assert_eq!(
            code(close(&|s| {
                s.peer_stream(uni(Side::Client, 0), false);
                s.data(uni(Side::Client, 0), &[0x03], false);
                s.data(uni(Side::Client, 0), &[0x40; MAX_DECODER_BYTES + 1], false);
            })),
            error::QPACK_DECODER_STREAM_ERROR
        );
        // A frame past the cap, and a buffer past the cap.
        assert_eq!(
            code(close(&|s| {
                s.peer_stream(bi(Side::Client, 0), true);
                let mut d = Vec::new();
                frames::put_varint(&mut d, frame::HEADERS);
                frames::put_varint(&mut d, frames::MAX_FRAME_PAYLOAD as u64 + 1);
                s.data(bi(Side::Client, 0), &d, false);
            })),
            error::H3_EXCESSIVE_LOAD
        );
        assert_eq!(
            code(close(&|s| {
                s.peer_stream(bi(Side::Client, 0), true);
                let mut d = Vec::new();
                frames::put_varint(&mut d, frame::HEADERS);
                frames::put_varint(&mut d, frames::MAX_FRAME_PAYLOAD as u64);
                d.extend(std::iter::repeat_n(0u8, MAX_STREAM_BUFFER));
                s.data(bi(Side::Client, 0), &d, false);
            })),
            error::H3_EXCESSIVE_LOAD
        );
        // A header block that does not decode; DATA before HEADERS; a second request stream; a push stream.
        assert_eq!(
            code(close(&|s| {
                s.peer_stream(bi(Side::Client, 0), true);
                let mut d = Vec::new();
                frames::put_frame(&mut d, frame::HEADERS, &[0x01, 0x00]);
                s.data(bi(Side::Client, 0), &d, false);
            })),
            error::QPACK_DECOMPRESSION_FAILED
        );
        assert_eq!(
            code(close(&|s| {
                s.peer_stream(bi(Side::Client, 0), true);
                s.data(bi(Side::Client, 0), &[0x00, 0x01, 0x00], false);
            })),
            error::H3_FRAME_UNEXPECTED
        );
        assert_eq!(
            code(close(&|s| {
                s.peer_stream(bi(Side::Client, 0), true);
                s.peer_stream(bi(Side::Client, 1), true);
            })),
            error::H3_STREAM_CREATION_ERROR
        );
        assert_eq!(
            code(close(&|s| {
                s.peer_stream(uni(Side::Client, 0), false);
                s.data(uni(Side::Client, 0), &[0x01], false);
            })),
            error::H3_STREAM_CREATION_ERROR
        );
        // An unknown stream type is stopped, not a close; a WebTransport stream too; and the stream is forgotten as it is
        // stopped, so a peer opening one after another holds no state here.
        for kind in [0x54u8, 0x21, 0x2f] {
            let mut server = Session::server(setup());
            server.local_streams(
                uni(Side::Server, 0),
                uni(Side::Server, 1),
                uni(Side::Server, 2),
            );
            while server.poll().is_some() {}
            for i in 0..(MAX_PEER_STREAMS as u64 * 4) {
                server.peer_stream(uni(Side::Client, i), false);
                server.data(uni(Side::Client, i), &[kind, 1, 2, 3], false);
                assert_eq!(
                    server.poll(),
                    Some(Action::Stop {
                        stream: uni(Side::Client, i),
                        code: error::H3_STREAM_CREATION_ERROR
                    })
                );
                assert!(server.end().is_none());
                server.data(uni(Side::Client, i), &[9; 100], true);
                assert_eq!(server.buffered(), 0);
                assert_eq!(server.tracked(), 0);
                assert!(server.end().is_none());
            }
        }
        // Too many streams tracked at once close the connection.
        assert_eq!(
            code(close(&|s| {
                for i in 0..=MAX_PEER_STREAMS as u64 {
                    s.peer_stream(uni(Side::Client, i), false);
                }
            })),
            error::H3_STREAM_CREATION_ERROR
        );
    }

    #[test]
    fn a_refusing_server_ends_the_clients_session_and_a_bad_answer_is_a_protocol_error() {
        let mut client = Session::client(dial(None));
        client.local_streams(
            uni(Side::Client, 0),
            uni(Side::Client, 1),
            uni(Side::Client, 2),
        );
        client.request_stream(bi(Side::Client, 0));
        client.peer_stream(uni(Side::Server, 0), false);
        let mut d = vec![0x00];
        frames::put_frame(
            &mut d,
            frame::SETTINGS,
            &frames::put_settings(&our_settings()),
        );
        client.data(uni(Side::Server, 0), &d, false);
        assert!(!client.waiting_for_settings());
        let mut answer = Vec::new();
        frames::put_frame(
            &mut answer,
            frame::HEADERS,
            &qpack::encode(&[(":status", "404")]),
        );
        client.data(bi(Side::Client, 0), &answer, true);
        assert_eq!(client.end(), Some(&End::Refused(404)));
        let mut client = Session::client(dial(None));
        client.local_streams(
            uni(Side::Client, 0),
            uni(Side::Client, 1),
            uni(Side::Client, 2),
        );
        client.request_stream(bi(Side::Client, 0));
        let mut answer = Vec::new();
        frames::put_frame(
            &mut answer,
            frame::HEADERS,
            &qpack::encode(&[("server", "x")]),
        );
        client.data(bi(Side::Client, 0), &answer, false);
        assert_eq!(
            client.end(),
            Some(&End::Protocol("an answer with no status"))
        );
        // A server's SETTINGS without WebTransport: the client gives up.
        let mut client = Session::client(dial(None));
        client.local_streams(
            uni(Side::Client, 0),
            uni(Side::Client, 1),
            uni(Side::Client, 2),
        );
        client.peer_stream(uni(Side::Server, 0), false);
        let mut d = vec![0x00];
        frames::put_frame(
            &mut d,
            frame::SETTINGS,
            &frames::put_settings(&[(setting::H3_DATAGRAM, 1)]),
        );
        client.data(uni(Side::Server, 0), &d, false);
        assert!(matches!(client.end(), Some(End::Protocol(_))));
    }
}
