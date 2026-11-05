//! The bytes a game socket reads, metered beneath the WebSocket layer.
//!
//! The WebSocket layer reads frames until a message is whole, inside one call: counting messages alone would let a peer send
//! endless frames that never finish one (empty continuations) at the speed of its link. So the stream the layer reads from
//! counts for it: each pass gives it a byte budget, after which it reports that nothing is waiting, and it follows the
//! frames' headers in the bytes it hands up, so that it can hold the peer to a cap of bytes a second, of frames a second
//! (every frame, control frames and continuations included) and of frames per message. A peer past a cap is marked; the
//! connection closes it. Beneath, the pipe meters the wire's bytes (under TLS, records that carry no plaintext count too).

use std::io::{self, ErrorKind, Read, Write};

use web_time::{Duration, Instant};

use super::tls::Pipe;

/// What a metered stream holds a peer to.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Caps {
    pub bytes_per_pass: usize,
    pub bytes_per_s: usize,
    pub frames_per_s: u32,
    pub frames_per_message: u32,
}

/// Why a peer was stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Over {
    /// More bytes or frames a second than the caps.
    Rate,
    /// More frames in one message than the cap, or a frame no client sends (unmasked, a reserved opcode).
    Frames,
}

/// Where the frame scanner is in the stream.
#[derive(Debug, Clone, Copy)]
enum Scan {
    /// Reading a header: the bytes so far and how many it needs.
    Header { got: [u8; 14], len: usize },
    /// Skipping a payload: the bytes left.
    Payload(u64),
}

/// A game socket's stream, metered (module docs).
pub(crate) struct Metered {
    pipe: Pipe,
    caps: Caps,
    budget: usize,
    window: Instant,
    bytes: usize,
    frames: u32,
    /// Frames of the message being sent (0 between messages).
    in_message: u32,
    scan: Scan,
    over: Option<Over>,
}

impl Metered {
    pub(crate) fn new(pipe: Pipe, caps: Caps, now: Instant) -> Metered {
        Metered {
            pipe,
            caps,
            budget: caps.bytes_per_pass,
            window: now,
            bytes: 0,
            frames: 0,
            in_message: 0,
            scan: Scan::Header {
                got: [0; 14],
                len: 0,
            },
            over: None,
        }
    }

    /// A new pass at `now`: the byte budget is full again.
    pub(crate) fn refill(&mut self, now: Instant) {
        self.pipe.refill(now);
        self.budget = self.caps.bytes_per_pass;
        if now.duration_since(self.window) >= Duration::from_secs(1) {
            self.window = now;
            self.bytes = 0;
            self.frames = 0;
        }
    }

    /// Whether the peer went past a cap (the wire's included).
    pub(crate) fn over(&self) -> Option<Over> {
        self.over.or(self.pipe.over().then_some(Over::Rate))
    }

    pub(crate) fn shut(&mut self) {
        self.pipe.shut();
    }

    /// Follows the frames' headers through `bytes` (masked client frames: 2 header bytes, 2 or 8 more for a long length, 4
    /// of mask).
    fn scan(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() && self.over.is_none() {
            match &mut self.scan {
                Scan::Payload(left) => {
                    let take = (*left).min(bytes.len() as u64);
                    *left -= take;
                    bytes = &bytes[take as usize..];
                    if *left == 0 {
                        self.scan = Scan::Header {
                            got: [0; 14],
                            len: 0,
                        };
                    }
                }
                Scan::Header { got, len } => {
                    got[*len] = bytes[0];
                    *len += 1;
                    bytes = &bytes[1..];
                    let need = header_len(&got[..*len]);
                    if *len < need {
                        continue;
                    }
                    let (head, n) = (*got, *len);
                    self.frame(&head[..n]);
                }
            }
        }
    }

    /// One whole frame header.
    fn frame(&mut self, header: &[u8]) {
        let fin = header[0] & 0x80 != 0;
        let opcode = header[0] & 0x0f;
        let masked = header[1] & 0x80 != 0;
        let payload = match header[1] & 0x7f {
            126 => u64::from(u16::from_be_bytes([header[2], header[3]])),
            127 => u64::from_be_bytes(header[2..10].try_into().unwrap_or([0xff; 8])),
            n => u64::from(n),
        };
        self.frames += 1;
        if self.frames > self.caps.frames_per_s {
            self.over = Some(Over::Rate);
        }
        match opcode {
            // A continuation.
            0 => {
                self.in_message += 1;
                if fin {
                    self.in_message = 0;
                }
            }
            1 | 2 => self.in_message = if fin { 0 } else { 1 },
            8..=10 => {}
            _ => self.over = Some(Over::Frames),
        }
        if !masked || self.in_message > self.caps.frames_per_message {
            self.over = Some(Over::Frames);
        }
        self.scan = if payload == 0 {
            Scan::Header {
                got: [0; 14],
                len: 0,
            }
        } else {
            Scan::Payload(payload)
        };
    }
}

/// The bytes a client frame's header takes, from its first bytes (at least 2 to know).
fn header_len(got: &[u8]) -> usize {
    let Some(&second) = got.get(1) else {
        return 2;
    };
    let length = match second & 0x7f {
        126 => 2,
        127 => 8,
        _ => 0,
    };
    let mask = if second & 0x80 != 0 { 4 } else { 0 };
    2 + length + mask
}

impl Read for Metered {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.over.is_some() || self.budget == 0 {
            return Err(ErrorKind::WouldBlock.into());
        }
        let room = buf.len().min(self.budget);
        let n = self.pipe.read(&mut buf[..room])?;
        self.budget -= n;
        self.bytes += n;
        if self.bytes > self.caps.bytes_per_s {
            self.over = Some(Over::Rate);
        }
        self.scan(&buf[..n]);
        Ok(n)
    }
}

impl Write for Metered {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pipe.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.pipe.flush()
    }
}
