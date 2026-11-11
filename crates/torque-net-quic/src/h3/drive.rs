//! Drives a [`Session`] on a `quinn_proto::Connection`: opens its streams, feeds it the peer's stream bytes and carries out
//! its actions. The server and the client endpoints hold one per connection that speaks `h3`.

use bytes::Bytes;
use quinn_proto::{Connection, Dir, ReadError, StreamEvent, StreamId, VarInt};

use super::frames::error;
use super::session::{Action, End, Session};

/// What came of a drive, for the endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing new.
    Quiet,
    /// The session was established just now: datagrams may flow.
    Established,
    /// The connection is to be closed with this code and reason.
    Close { code: u64, reason: String },
}

/// One connection's session and the bookkeeping of its streams.
#[derive(Debug)]
pub struct Driver {
    pub session: Session,
    readable: Vec<StreamId>,
    prefix: Vec<u8>,
    established: bool,
}

impl Driver {
    /// A driver of `session`, whose own streams it opens on `conn` now (three unidirectional; a client's bidirectional
    /// request stream too). None when the peer allows fewer streams than the session needs.
    #[must_use]
    pub fn open(mut session: Session, conn: &mut Connection, client: bool) -> Option<Driver> {
        let mut streams = conn.streams();
        let control = streams.open(Dir::Uni)?;
        let encoder = streams.open(Dir::Uni)?;
        let decoder = streams.open(Dir::Uni)?;
        let request = if client {
            Some(streams.open(Dir::Bi)?)
        } else {
            None
        };
        session.local_streams(control, encoder, decoder);
        if let Some(request) = request {
            session.request_stream(request);
        }
        Some(Driver {
            session,
            // The server's answer comes on the client's own request stream.
            readable: request.into_iter().collect(),
            prefix: Vec::new(),
            established: false,
        })
    }

    /// Whether the session is established and datagrams may flow.
    #[must_use]
    pub fn established(&self) -> bool {
        self.established
    }

    /// The bytes in front of every datagram of the session.
    #[must_use]
    pub fn prefix_len(&self) -> usize {
        self.prefix.len()
    }

    /// A stream event of the connection: new streams are accepted into the session, readable ones noted.
    pub fn stream_event(&mut self, conn: &mut Connection, event: StreamEvent) {
        match event {
            StreamEvent::Opened { dir } => {
                while let Some(id) = conn.streams().accept(dir) {
                    self.session.peer_stream(id, dir == Dir::Bi);
                    self.readable.push(id);
                }
            }
            StreamEvent::Readable { id } => {
                if !self.readable.contains(&id) {
                    self.readable.push(id);
                }
            }
            StreamEvent::Writable { .. }
            | StreamEvent::Finished { .. }
            | StreamEvent::Stopped { .. }
            | StreamEvent::Available { .. } => {}
        }
    }

    /// Reads what the noted streams have, runs the session's actions on `conn`, and says what came of it.
    pub fn run(&mut self, conn: &mut Connection) -> Outcome {
        for id in std::mem::take(&mut self.readable) {
            self.read(conn, id);
        }
        let mut outcome = Outcome::Quiet;
        while let Some(action) = self.session.poll() {
            match action {
                Action::Write { stream, data } => {
                    // Everything a session writes is a few hundred bytes, under any peer's first window: a short write is a
                    // peer that gave none, and the session cannot go on.
                    match conn.send_stream(stream).write(&data) {
                        Ok(n) if n == data.len() => {}
                        _ => {
                            return Outcome::Close {
                                code: error::H3_GENERAL_PROTOCOL_ERROR,
                                reason: "the peer gave no room for the session's stream"
                                    .to_string(),
                            };
                        }
                    }
                }
                Action::Finish(stream) => {
                    let _ = conn.send_stream(stream).finish();
                }
                Action::Stop { stream, code } => {
                    let _ = conn
                        .recv_stream(stream)
                        .stop(VarInt::from_u64(code).unwrap_or(VarInt::MAX));
                }
                Action::Close { code, reason } => {
                    return Outcome::Close {
                        code,
                        reason: reason.to_string(),
                    };
                }
            }
        }
        if !self.established && self.session.established().is_some() {
            self.established = true;
            self.prefix = self.session.datagram_prefix();
            outcome = Outcome::Established;
        }
        match self.session.end() {
            None => outcome,
            Some(End::Closed { code, reason }) => Outcome::Close {
                code: error::H3_NO_ERROR,
                reason: if reason.is_empty() {
                    format!("the session closed ({code})")
                } else {
                    format!("the session closed ({code}): {reason}")
                },
            },
            Some(End::Refused(status)) => Outcome::Close {
                code: error::H3_REQUEST_REJECTED,
                reason: format!("the server refused the WebTransport session: {status}"),
            },
            Some(End::Protocol(reason)) => Outcome::Close {
                code: error::H3_GENERAL_PROTOCOL_ERROR,
                reason: (*reason).to_string(),
            },
        }
    }

    fn read(&mut self, conn: &mut Connection, id: StreamId) {
        let mut stream = conn.recv_stream(id);
        let Ok(mut chunks) = stream.read(true) else {
            return;
        };
        let mut fin = false;
        let mut reset = false;
        let mut got = Vec::new();
        loop {
            match chunks.next(usize::MAX) {
                Ok(Some(chunk)) => got.push(chunk.bytes),
                Ok(None) => {
                    fin = true;
                    break;
                }
                Err(ReadError::Blocked) => break,
                Err(ReadError::Reset(_)) => {
                    reset = true;
                    break;
                }
            }
        }
        let _ = chunks.finalize();
        for bytes in &got {
            self.session.data(id, bytes, false);
        }
        if fin {
            self.session.data(id, &[], true);
        } else if reset {
            self.session.reset(id);
        }
    }

    /// An engine datagram as the session sends it: the quarter stream id in front.
    #[must_use]
    pub fn wrap(&self, data: &[u8]) -> Bytes {
        let mut out = Vec::with_capacity(self.prefix.len() + data.len());
        out.extend_from_slice(&self.prefix);
        out.extend_from_slice(data);
        Bytes::from(out)
    }

    /// The engine datagram inside a datagram of the connection, if it is the session's.
    #[must_use]
    pub fn unwrap(&self, data: &Bytes) -> Option<Bytes> {
        if !self.established {
            return None;
        }
        let payload = self.session.unwrap_datagram(data)?;
        Some(data.slice(data.len() - payload.len()..))
    }
}
