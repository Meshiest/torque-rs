//! [`Http`]: HTTPObject's request and its reading of the answer (`HTTPObject::onConnected`, `onReceive` and `processLine`).
//!
//! The answer is read line by line through TCPObject's splitting: the status line and the headers are taken in (only
//! `Transfer-Encoding: chunked`, written exactly so in any case, is looked at), and each line of the body is the script's
//! `onLine`. A chunked body is cut at each chunk's size, and a line that runs across a chunk's end goes on in the next chunk.
//! Nothing ends the answer but the server closing the connection (or the last chunk, after which the rest is dropped).

use crate::lines::{Lines, TooLong};

/// The request's text is cut at this (TGE builds it in buffers of this size).
const REQUEST_BUFFER: usize = 8192;

/// What `get` or `post` asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    /// The `Host` header: the address's host, without its port.
    pub(crate) host: String,
    pub(crate) path: Vec<u8>,
    pub(crate) query: Option<Vec<u8>>,
    /// The body of a `post`, sent as a form.
    pub(crate) post: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Parse {
    #[default]
    StatusLine,
    Header,
    ChunkHeader,
    Body,
    Done,
}

/// The state of one HTTPObject.
#[derive(Debug, Default)]
pub(crate) struct Http {
    pub(crate) request: Option<Request>,
    parse: Parse,
    chunked: bool,
    /// What is left of the chunk being read (wraps as TGE's `U32` does when the body is not chunked).
    chunk: u32,
    /// The piece of a line that waits across a chunk header (`mBufferSave`).
    saved: Option<Vec<u8>>,
}

/// One `onReceive`: how many bytes it took, and the body line it gives the script, if any.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Received {
    pub(crate) used: usize,
    pub(crate) line: Option<Vec<u8>>,
}

/// TGE's `getHexVal`: any letter counts, `g` as 16 and on to `z` as 35.
fn hex_value(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some(u32::from(c - b'0')),
        b'A'..=b'Z' => Some(u32::from(c - b'A') + 10),
        b'a'..=b'z' => Some(u32::from(c - b'a') + 10),
        _ => None,
    }
}

/// Whether `expandPath` writes the byte as `%XX`: the controls and the space, every byte past 127, and `"_'#$%&+-~`.
fn escaped(c: u8) -> bool {
    c <= b' ' || c >= 0x80 || b"\"_'#$%&+-~".contains(&c)
}

/// `HTTPObject::expandPath`: the path with the bytes [`escaped`] says written as `%XX`, cut where TGE's buffer ends.
pub(crate) fn expand_path(path: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = Vec::with_capacity(path.len());
    for &c in path {
        if out.len() >= REQUEST_BUFFER - 3 {
            break;
        }
        if escaped(c) {
            out.extend_from_slice(&[b'%', HEX[usize::from(c >> 4)], HEX[usize::from(c & 0xF)]]);
        } else {
            out.push(c);
        }
    }
    out
}

impl Http {
    /// The request sent once the connection is open (`onConnected`), and the reading of the answer starts over. A `post`
    /// sends its body as a form (`Content-Type: application/x-www-form-urlencoded`).
    pub(crate) fn begin(&mut self) -> Vec<u8> {
        self.parse = Parse::StatusLine;
        self.chunked = false;
        let Some(request) = &self.request else {
            return Vec::new();
        };
        let target = match &request.query {
            Some(query) => {
                let mut joined = [&request.path[..], b"?", query].concat();
                joined.truncate(REQUEST_BUFFER - 1);
                expand_path(&joined)
            }
            None => expand_path(&request.path),
        };
        let mut text = Vec::with_capacity(target.len() + 64);
        match &request.post {
            None => {
                text.extend_from_slice(b"GET ");
                text.extend_from_slice(&target);
                text.extend_from_slice(b" HTTP/1.1\r\nHost: ");
                text.extend_from_slice(request.host.as_bytes());
                text.extend_from_slice(b"\r\n\r\n");
            }
            Some(body) => {
                text.extend_from_slice(b"POST ");
                text.extend_from_slice(&target);
                text.extend_from_slice(b" HTTP/1.1\r\nHost: ");
                text.extend_from_slice(request.host.as_bytes());
                text.extend_from_slice(
                    format!(
                        "\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                );
                text.extend_from_slice(body);
            }
        }
        text
    }

    /// Takes the request out once the connection fails or ends.
    pub(crate) fn end(&mut self) {
        self.request = None;
    }

    /// One `onReceive` over `buffer` (what is left of a read).
    pub(crate) fn receive(
        &mut self,
        lines: &mut Lines,
        buffer: &[u8],
        max: usize,
    ) -> Result<Received, TooLong> {
        match self.parse {
            Parse::Done => Ok(Received {
                used: buffer.len(),
                line: None,
            }),
            Parse::Body => {
                let chunk = usize::try_from(self.chunk).unwrap_or(usize::MAX);
                if self.chunked && buffer.len() >= chunk {
                    let got = self.data(lines, &buffer[..chunk], max)?;
                    // `used` is at most `chunk`, which came from the `u32`.
                    self.chunk = self
                        .chunk
                        .wrapping_sub(u32::try_from(got.used).unwrap_or(u32::MAX));
                    if self.chunk == 0 {
                        if lines.is_buffered() {
                            self.saved = lines.take();
                        }
                        self.parse = Parse::ChunkHeader;
                    }
                    Ok(got)
                } else {
                    let got = self.data(lines, buffer, max)?;
                    // Unchunked, the count only wraps as TGE's does and is never read.
                    self.chunk = self
                        .chunk
                        .wrapping_sub(u32::try_from(got.used).unwrap_or(u32::MAX));
                    Ok(got)
                }
            }
            Parse::StatusLine | Parse::Header | Parse::ChunkHeader => self.data(lines, buffer, max),
        }
    }

    /// `parseLine` from the start of `buffer`, the line it ends taken by [`process_line`](Self::process_line).
    fn data(&mut self, lines: &mut Lines, buffer: &[u8], max: usize) -> Result<Received, TooLong> {
        let step = lines.parse_line(buffer, 0, max)?;
        let line = step.line.and_then(|line| self.process_line(lines, line));
        Ok(Received {
            used: step.next,
            line,
        })
    }

    /// `HTTPObject::processLine`: the line the script gets as `onLine`, if any.
    pub(crate) fn process_line(&mut self, lines: &mut Lines, line: Vec<u8>) -> Option<Vec<u8>> {
        match self.parse {
            Parse::StatusLine => {
                self.parse = Parse::Header;
                None
            }
            Parse::Header => {
                if line.eq_ignore_ascii_case(b"transfer-encoding: chunked") {
                    self.chunked = true;
                }
                if line.is_empty() {
                    self.parse = if self.chunked {
                        Parse::ChunkHeader
                    } else {
                        Parse::Body
                    };
                }
                None
            }
            Parse::ChunkHeader => {
                if line.is_empty() {
                    return None;
                }
                self.chunk = line
                    .iter()
                    .map_while(|&c| hex_value(c))
                    .fold(0u32, |size, digit| {
                        size.wrapping_mul(16).wrapping_add(digit)
                    });
                if let Some(saved) = self.saved.take() {
                    lines.restore(saved);
                }
                if self.chunk != 0 {
                    self.parse = Parse::Body;
                    return None;
                }
                self.parse = Parse::Done;
                let last = lines.finish_last_line()?;
                self.process_line(lines, last)
            }
            Parse::Body | Parse::Done => Some(line),
        }
    }
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
