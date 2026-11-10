//! What a realm's ops do in the engine, and why a realm stops. The engine gives a [`Session`](crate::Session) an
//! [`Effects`] for each call that may apply ops; the session has checked every op against the format and the grants
//! before it reaches here.

use std::fmt;

use torque_client_script_format::{Tier, Value};

use crate::runner::Trap;

/// An op a checked guest never sends: the realm is stopped for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Violation {
    /// A flush with more records or bytes than the format allows.
    FlushCaps,
    /// A record that does not follow the format's framing.
    Framing,
    /// An op of a tier id that names no tier.
    NoTier,
    /// An op of a tier the realm was not granted.
    Ungranted,
    /// An op of a tier the guest had been told it lost.
    Revoked,
    /// An op of a tier the format has no ops for.
    NotInFormat,
    /// An op of a kind its tier does not have.
    NoKind,
    /// A `realm` or `net` op whose body does not follow the format (a level or limit of no kind included).
    Body,
    /// A log line with hidden characters, or past its length.
    LogNotPlain,
    /// A message or call arguments past their size cap.
    TooLarge,
    /// Messages and calls past their rate.
    TooFast,
    /// A call id out of order or used twice.
    CallOrder,
    /// An RPC name that is not plain.
    RpcName,
    /// More calls waiting than the caps allow.
    TooManyCalls,
    /// The GUI's applier refused a `ui` op, for this reason.
    Ui(&'static str),
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Violation::FlushCaps => "a flush past its caps",
            Violation::Framing => "a flush that does not follow the format",
            Violation::NoTier => "an op of no tier",
            Violation::Ungranted => "an op of a tier the realm was not granted",
            Violation::Revoked => "an op of a tier the realm was told it lost",
            Violation::NotInFormat => "an op of a tier format 1 does not have",
            Violation::NoKind => "an op of no kind",
            Violation::Body => "an op that does not follow the format",
            Violation::LogNotPlain => "a log line the guest did not make plain",
            Violation::TooLarge => "a message or call past its size cap",
            Violation::TooFast => "messages and calls past their rate",
            Violation::CallOrder => "a call id out of order",
            Violation::RpcName => "an RPC name that is not plain",
            Violation::TooManyCalls => "more calls waiting than the caps allow",
            Violation::Ui(why) => why,
        })
    }
}

/// A log line's level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    /// Information.
    Info,
    /// A warning (dropped lines, a refused request).
    Warning,
    /// A script error.
    Error,
}

/// What became of a `net.call`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallRoute {
    /// It went to the server; [`Session::reply`](crate::Session::reply) hands back its answer.
    Sent,
    /// The server did not mark that RPC for the realm: the session answers it with a refusal.
    Refused,
}

/// Which of the host's calls the guest refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Making the realm (`torque_realm_new`).
    New,
    /// Keeping a script file, for a reason other than the file itself.
    Script,
    /// Running a file, for a reason other than the file not being there.
    Run,
    /// A frame's event batch.
    Frame,
}

/// Why a realm stopped. A stopped realm runs nothing again on its connection; a reconnect makes a fresh one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Its instance trapped (the deadline, the memory cap, a fault).
    Trapped(Trap),
    /// It sent an op a checked guest never sends.
    Violation(Violation),
    /// It hit its limits, or ran entries too long, too often (the strike rule).
    Strikes,
    /// The guest refused what the host handed it (a host fault, or a guest that is not the one it claims to be).
    Refused(Refusal),
}

impl fmt::Display for Stop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Stop::Trapped(trap) => trap.fmt(f),
            Stop::Violation(why) => write!(f, "it broke the rules ({why})"),
            Stop::Strikes => f.write_str("it ran out of time or memory too often"),
            Stop::Refused(what) => write!(f, "it refused the engine's call ({what:?})"),
        }
    }
}

/// The engine's side of one realm: where its checked ops go.
pub trait Effects {
    /// A log line of the realm's, already plain text within the format's caps; the engine tags it with the server.
    fn log(&mut self, level: LogLevel, line: &str);

    /// Checks `ui` records (a flush holding only them, in their order) without applying them: the GUI's applier is the
    /// enforcement point of their handles, props and caps. The session calls this with all of a flush's `ui` records
    /// before it applies anything of that flush.
    ///
    /// # Errors
    ///
    /// A [`Violation`] stops the realm, and nothing of the flush applies.
    fn check_ui(&mut self, records: &[u8]) -> Result<(), Violation>;

    /// Applies `ui` records that [`Effects::check_ui`] passed (a flush's may come in several pieces, in order).
    ///
    /// # Errors
    ///
    /// A [`Violation`] stops the realm; the flush's `net` ops, held until its `ui` ops applied, are not sent.
    fn apply_ui(&mut self, records: &[u8]) -> Result<(), Violation>;

    /// A `net.send` for the realm's server.
    fn send(&mut self, message: Value);

    /// A `net.call` of the server's RPC `name`.
    fn call(&mut self, id: u32, name: &str, args: Value) -> CallRoute;

    /// The player took `tier` away: remove all it did (in this frame), and hand it nothing more.
    fn revoke(&mut self, tier: Tier);

    /// The realm is gone (stopped or closed): remove all it did.
    fn teardown(&mut self);

    /// The realm stopped, for the notice the player sees.
    fn stopped(&mut self, why: &Stop);
}
