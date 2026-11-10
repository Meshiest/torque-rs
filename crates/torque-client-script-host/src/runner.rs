//! What runs a realm's guest: one instance, called one entry at a time. Natively that is Wasmtime on the engine's thread
//! (an entry returns before `start` does); on the web a Worker (an entry's result arrives on a later poll). The
//! [`Session`](crate::Session) drives either the same way.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use torque_client_script_format::abi::Config;

/// One call into the guest's exports.
#[derive(Clone, Debug)]
pub enum Entry {
    /// `torque_realm_new`: makes the instance's one realm.
    New(Config),
    /// `torque_realm_add_script`: keeps one script file of a mod.
    AddScript {
        /// The mod's id.
        mod_id: Arc<str>,
        /// The file's path within the mod.
        path: Arc<str>,
        /// The file's bytes, as the server's verified file list holds them.
        source: Arc<[u8]>,
    },
    /// `torque_realm_run`: runs a kept file.
    Run {
        /// The microseconds since the realm was made.
        now_us: u64,
        /// The mod's id.
        mod_id: Arc<str>,
        /// The file's path within the mod.
        path: Arc<str>,
    },
    /// `torque_realm_frame`: one frame with an event batch.
    Frame {
        /// The microseconds since the realm was made.
        now_us: u64,
        /// The batch's records.
        events: Vec<u8>,
    },
}

impl Entry {
    /// Whether the entry leaves a flush (`torque_realm_ops`) for the host to read.
    pub fn has_flush(&self) -> bool {
        matches!(self, Entry::Run { .. } | Entry::Frame { .. })
    }

    /// The number the web runner's messages give the entry.
    pub fn code(&self) -> u32 {
        match self {
            Entry::New(_) => 0,
            Entry::AddScript { .. } => 1,
            Entry::Run { .. } => 2,
            Entry::Frame { .. } => 3,
        }
    }
}

/// What an entry returned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    /// Its status (`torque_client_script_format::abi::OK` ...).
    pub status: i32,
    /// Its flush (empty for the entries that leave none).
    pub ops: Vec<u8>,
    /// The events still queued in the guest for a later frame.
    pub pending: u32,
    /// How long the entry ran, measured by the runner around its call (a Worker's own time on the web).
    pub elapsed: Duration,
    /// Whether the entry ran past the long-entry time: natively flagged by the epoch while it ran.
    pub long: bool,
}

/// Why an instance is gone. After any of these nothing of it is called again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trap {
    /// An entry ran past its wall-clock deadline (the epoch natively, the Worker's watchdog on the web).
    Deadline,
    /// The guest needed more linear memory than its cap.
    Memory,
    /// Anything else: a guest fault, a module that is not a realm guest, a flush past the format's caps.
    Fault(String),
}

impl fmt::Display for Trap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Trap::Deadline => f.write_str("it ran past its deadline"),
            Trap::Memory => f.write_str("it needed more memory than it may have"),
            Trap::Fault(why) => write!(f, "it failed ({why})"),
        }
    }
}

/// An entry's outcome.
pub type Outcome = Result<Reply, Trap>;

/// The caps of one instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstanceLimits {
    /// The most linear memory it may have, in bytes.
    pub memory: usize,
    /// How long one entry may run before it is stopped.
    pub deadline: Duration,
    /// How long an entry may run before it counts as a strike.
    pub long_entry: Duration,
}

/// One instance of a realm's guest.
///
/// At most one entry is in flight: [`Runner::start`] is called only when [`Runner::poll`] has handed back the previous
/// entry's outcome. After an outcome of `Err` the instance is gone and every later entry's outcome is `Err` too.
pub trait Runner {
    /// Starts `entry`.
    fn start(&mut self, entry: Entry);

    /// The started entry's outcome, once there is one (taken: the next poll has none until another entry starts).
    fn poll(&mut self) -> Option<Outcome>;
}

/// The outcome every entry of a gone instance has.
#[cfg(any(feature = "wasmtime", feature = "web"))]
pub(crate) fn gone() -> Trap {
    Trap::Fault("the instance was thrown away".into())
}

/// A runner whose instance never started: every entry's outcome is the trap that stopped it, so a session over it
/// stops on its first frame and tells the player why.
#[derive(Debug)]
pub struct Unstarted(pub Trap);

impl Runner for Unstarted {
    fn start(&mut self, _entry: Entry) {}

    fn poll(&mut self) -> Option<Outcome> {
        Some(Err(self.0.clone()))
    }
}
