//! What a server holds one kind of upload to: [`UploadSpec`], and why it may refuse one ([`Refusal`]).

use std::fmt;
use std::time::Duration;

use torque_net::modsync::size_text;

/// The most bytes any upload may be, whatever a spec says.
pub const MAX_UPLOAD_BYTES: u64 = 256 << 20;

/// The most bytes of metadata, whatever a spec says (the begin request is one RPC message of at most 1,024 bytes).
pub const MAX_META_BYTES: usize = 768;

/// The longest name of an upload kind.
pub const MAX_KIND_NAME: usize = 40;

/// How much one player may upload of one kind in a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quota {
    /// The most bytes accepted in `per`.
    pub bytes: u64,
    /// The window the quota covers.
    pub per: Duration,
    /// The most uploads accepted in `per`.
    pub files: u32,
}

/// The limits of one kind of upload. A server asks nothing of a client that is not in them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadSpec {
    /// The most bytes of one upload.
    pub max_bytes: u64,
    /// What one player may send in a time. A player is the auth identity, else the connection.
    pub quota: Quota,
    /// The bytes a second the server requests from one connection.
    pub rate: u32,
    /// The uploads one player may have going at once.
    pub concurrent: u32,
    /// The declared bytes of this kind's uploads that hold their room together. An upload quiet (or waiting to resume) for
    /// `idle` stops counting until its data comes or it is resumed, and is then taken back only if there is room; what is
    /// held staged is bounded by the staging's own caps.
    pub server_in_flight: u64,
    /// The bytes a second the server requests from all players together.
    pub server_rate: u64,
    /// How long an upload that waits for the client (for its block list, or for runs it asked for) may go without a word from its
    /// connection. Bytes the connection delivers buy time (the time a link of 1 KiB a second needs for them), so a slow link that
    /// keeps delivering is alive and a trickle is not. Quiet for this long, the upload gives up its room against `server_in_flight`
    /// and goes last in line for runs; this long again later it is ended. A waiting upload gives up its room after this too.
    pub idle: Duration,
    /// How long an upload may take from its accept.
    pub total: Duration,
    /// How long the server keeps what it has of an upload after its connection ends.
    pub resume: Duration,
    /// The most bytes of the metadata that describes an upload.
    pub meta_max: usize,
    /// A permission the player needs to start one, else anyone may.
    pub permission: Option<String>,
}

impl Default for UploadSpec {
    fn default() -> UploadSpec {
        UploadSpec {
            max_bytes: 16 << 20,
            quota: Quota {
                bytes: 64 << 20,
                per: Duration::from_secs(86_400),
                files: 32,
            },
            rate: 128 << 10,
            concurrent: 1,
            server_in_flight: 64 << 20,
            server_rate: 4 << 20,
            idle: Duration::from_secs(30),
            total: Duration::from_secs(900),
            resume: Duration::from_secs(300),
            meta_max: 512,
            permission: None,
        }
    }
}

/// Why a spec cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpecError(pub &'static str);

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for SpecError {}

impl UploadSpec {
    /// Whether the limits make sense: every number in its range, and room for at least one upload.
    ///
    /// # Errors
    /// A [`SpecError`] naming the first limit out of range.
    pub fn validate(&self) -> Result<(), SpecError> {
        if self.max_bytes == 0 || self.max_bytes > MAX_UPLOAD_BYTES {
            return Err(SpecError("max_bytes is 1 to 256 MiB"));
        }
        if self.quota.files == 0 || self.quota.bytes == 0 || self.quota.per.is_zero() {
            return Err(SpecError("a quota needs bytes, files and a window"));
        }
        if self.rate == 0 || self.server_rate == 0 {
            return Err(SpecError("rate and server_rate are at least 1"));
        }
        if !(1..=16).contains(&self.concurrent) {
            return Err(SpecError("concurrent is 1 to 16"));
        }
        if self.server_in_flight < self.max_bytes {
            return Err(SpecError("server_in_flight holds at least one upload"));
        }
        if self.idle.is_zero() || self.total.is_zero() {
            return Err(SpecError("idle and total are not zero"));
        }
        if self.meta_max > MAX_META_BYTES {
            return Err(SpecError("meta_max is at most 768"));
        }
        Ok(())
    }
}

/// Why a server refuses to take an upload. The text is for the player.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    /// The server has no upload of that name.
    UnknownKind,
    /// The player lacks the permission the kind needs.
    NotPermitted,
    /// The content has no bytes.
    Empty,
    /// The content is above the kind's limit.
    TooLarge {
        /// The declared size.
        size: u64,
        /// The kind's limit.
        max: u64,
    },
    /// The metadata is above the kind's limit.
    MetaTooLarge {
        /// The metadata's size.
        len: usize,
        /// The kind's limit.
        max: usize,
    },
    /// The upload would take the player past the quota's bytes.
    QuotaBytes {
        /// What the quota allows.
        limit: u64,
        /// What is left of it now.
        left: u64,
        /// The window.
        per: Duration,
    },
    /// The player has used the quota's uploads.
    QuotaFiles {
        /// What the quota allows.
        limit: u32,
        /// The window.
        per: Duration,
    },
    /// The player has all their uploads going.
    Concurrent {
        /// What the kind allows at once.
        max: u32,
    },
    /// The player already has as many bytes waiting on the server as one player may.
    HoldingTooMuch {
        /// What one player may have staged.
        limit: u64,
    },
    /// The server holds all the staged bytes it allows for the kind.
    ServerFull,
    /// The server cannot stage the upload now (its memory and folder for staging are in use).
    ServerBusy,
    /// The request was not one an honest client makes.
    Invalid(&'static str),
    /// The kind's own rules refused it, with their reason.
    Rejected(String),
}

/// A duration a player reads (`24 hours`).
pub fn duration_text(d: Duration) -> String {
    let secs = d.as_secs();
    let (n, unit) = if secs >= 7_200 {
        (secs / 3_600, "hours")
    } else if secs >= 120 {
        (secs / 60, "minutes")
    } else {
        (secs, "seconds")
    };
    format!("{n} {unit}")
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::UnknownKind => f.write_str("The server does not take that kind of upload."),
            Refusal::NotPermitted => f.write_str("You are not allowed to upload that."),
            Refusal::Empty => f.write_str("There is nothing to upload."),
            Refusal::TooLarge { size, max } => write!(
                f,
                "That is {} and this kind of upload takes at most {}.",
                size_text(*size),
                size_text(*max)
            ),
            Refusal::MetaTooLarge { len, max } => write!(
                f,
                "The description is {len} bytes and at most {max} are taken."
            ),
            Refusal::QuotaBytes { limit, left, per } => write!(
                f,
                "That would go over your limit of {} every {}; you have {} left.",
                size_text(*limit),
                duration_text(*per),
                size_text(*left)
            ),
            Refusal::QuotaFiles { limit, per } => write!(
                f,
                "You can send at most {limit} uploads every {}.",
                duration_text(*per)
            ),
            Refusal::Concurrent { max } => write!(
                f,
                "You already have {max} upload{} going; wait for it to finish.",
                if *max == 1 { "" } else { "s" }
            ),
            Refusal::HoldingTooMuch { limit } => write!(
                f,
                "You already have {} of uploads waiting on the server; let them finish first.",
                size_text(*limit)
            ),
            Refusal::ServerFull => {
                f.write_str("The server is taking too many uploads right now; try again later.")
            }
            Refusal::ServerBusy => {
                f.write_str("The server cannot hold that upload right now; try again later.")
            }
            Refusal::Invalid(why) => write!(f, "The upload request was not valid: {why}."),
            Refusal::Rejected(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for Refusal {}

#[cfg(test)]
#[path = "spec_tests.rs"]
mod tests;
