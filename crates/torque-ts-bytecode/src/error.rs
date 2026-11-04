use std::fmt;

/// Why a DSO file could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DsoError {
    /// The input ended while reading `what`.
    UnexpectedEof { what: &'static str, offset: usize },
    /// A declared count or length cannot fit in the bytes that remain.
    TooLarge {
        what: &'static str,
        declared: u64,
        remaining: usize,
    },
    /// Bytes remain after the identifier table.
    TrailingBytes { offset: usize, extra: usize },
}

impl fmt::Display for DsoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DsoError::UnexpectedEof { what, offset } => {
                write!(f, "unexpected end of file reading {what} at byte {offset}")
            }
            DsoError::TooLarge {
                what,
                declared,
                remaining,
            } => write!(
                f,
                "{what} declares {declared} entries but only {remaining} bytes remain"
            ),
            DsoError::TrailingBytes { offset, extra } => write!(
                f,
                "{extra} unexpected bytes after the identifier table (at byte {offset})"
            ),
        }
    }
}

impl std::error::Error for DsoError {}
