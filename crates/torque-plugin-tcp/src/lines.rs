//! [`Lines`]: TCPObject's line splitting (`parseLine` and `finishLastLine`).
//!
//! A line ends at `\n` or at a NUL byte, and one `\r` before the end is dropped. Bytes after the last line end wait for the
//! next read; the rest of a line split across reads keeps its `\r` check. What waits at the disconnect is the last line, sent
//! without its `\r` dropped.

/// The bytes of a line that has not ended yet. `Some` of an empty buffer differs from `None`, as TGE's `mBuffer` that is
/// allocated but holds nothing differs from no buffer: it makes the next piece go through the buffer.
#[derive(Debug, Default)]
pub(crate) struct Lines {
    partial: Option<Vec<u8>>,
}

/// A line longer than the cap: the connection is dropped and the line is not delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TooLong;

/// One `parseLine`: how far into the buffer it got, and the line it ended, if any.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Step {
    pub(crate) next: usize,
    pub(crate) line: Option<Vec<u8>>,
}

fn strip_cr(mut line: Vec<u8>) -> Vec<u8> {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    line
}

impl Lines {
    /// Reads `buffer` from `start` up to the first line end (included): the line it ends, or else the bytes kept for the next
    /// read. `max` caps a line's length.
    pub(crate) fn parse_line(
        &mut self,
        buffer: &[u8],
        start: usize,
        max: usize,
    ) -> Result<Step, TooLong> {
        let rest = buffer.get(start..).unwrap_or_default();
        let end = rest.iter().position(|&b| b == b'\n' || b == 0);
        let piece = &rest[..end.unwrap_or(rest.len())];
        let line = match (&mut self.partial, end) {
            (None, Some(_)) => Some(strip_cr(piece.to_vec())),
            (partial, _) => {
                let kept = partial.get_or_insert_with(Vec::new);
                if kept.len().saturating_add(piece.len()) > max {
                    self.partial = None;
                    return Err(TooLong);
                }
                kept.extend_from_slice(piece);
                end.and_then(|_| self.partial.take()).map(strip_cr)
            }
        };
        if line.as_ref().is_some_and(|l| l.len() > max) {
            return Err(TooLong);
        }
        let next = match end {
            Some(at) => start + at + 1,
            None => buffer.len(),
        };
        Ok(Step { next, line })
    }

    /// The bytes waiting when the connection ends, as a last line (`finishLastLine`): none when nothing waits.
    pub(crate) fn finish_last_line(&mut self) -> Option<Vec<u8>> {
        self.partial.take_if(|p| !p.is_empty())
    }

    /// Whether bytes of a line wait (an empty buffer counts, see [`Lines`]).
    pub(crate) fn is_buffered(&self) -> bool {
        self.partial.is_some()
    }

    /// Takes the waiting buffer out (HTTPObject keeps it across a chunk header).
    pub(crate) fn take(&mut self) -> Option<Vec<u8>> {
        self.partial.take()
    }

    /// Puts a buffer taken with [`take`](Self::take) back.
    pub(crate) fn restore(&mut self, partial: Vec<u8>) {
        self.partial = Some(partial);
    }

    /// Drops what waits.
    pub(crate) fn clear(&mut self) {
        self.partial = None;
    }
}

#[cfg(test)]
#[path = "lines_tests.rs"]
mod tests;
